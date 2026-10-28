//! Payment streams for clients (specification §5.31 to §5.34).
//!
//! A payer escrows funds for one payee and keeps the secret top of a hash
//! chain. Each payment reveals the next chain element; the payee checks it
//! with one hash against the last element it accepted, and settles many
//! payments in one transaction. Nothing per payment touches the ledger.
//!
//! [`PayerBook`] keeps a payer's chains and never reveals an element for
//! two different payments. [`StreamDesk`] is the payee's side: it accepts
//! payments, decides when to settle, and sends the settlements.
//!
//! **Checkpoints.** A chain opened with checkpoints (spec v1.7) lets the
//! payee settle any number of payments for a bounded number of hashes: the
//! payer attaches a [`Jump`] to the first payment past each checkpoint, the
//! desk checks it once, and settles with it instead of walking back to the
//! head. Without checkpoints, a settlement walks one hash per step.
//!
//! **What each side risks.** The payer's funds are in escrow until the
//! stream's last Pulse, so a payee that accepted a payment is paid as long
//! as it settles in time: the desk refuses payments from a stream that ends
//! within its margin, and settles everything once a stream gets that close.
//! The payer risks what the payee may take for requests it does not serve,
//! one request at a time, as with any pay-before-serve protocol.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sion_core::hash::Hash32;
use sion_core::ids::{AccountId, AssetId, ObjectId, Pulse, TxDigest};
use sion_core::Amount;
use sion_exec::stream::{
    jump_hashes, verify_jump, walk, Jump, PayerChain, Stream, MAX_SETTLE_STEPS,
};
use sion_exec::{ChainConfig, ExecStatus, ObjectData, TxKind, Wallet};

use crate::error::CliError;
use crate::ledger::{Ledger, LedgerExt, TxState};
use crate::session::RemoteView;

/// One payment from a stream: the chain element at `position`.
///
/// Written as `<stream>:<position>:<element>` (hex, decimal, hex).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Payment {
    /// The stream.
    pub stream: ObjectId,
    /// The chain position this element pays up to.
    pub position: u64,
    /// The chain element at `position`.
    pub value: Hash32,
}

impl fmt::Display for Payment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{}", self.stream, self.position, self.value)
    }
}

impl FromStr for Payment {
    type Err = CliError;

    fn from_str(text: &str) -> Result<Self, CliError> {
        let bad = || {
            CliError::Usage(format!(
                "{text:?} is not a payment (<stream>:<position>:<element>)"
            ))
        };
        let mut parts = text.trim().split(':');
        let (Some(stream), Some(position), Some(value), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(bad());
        };
        Ok(Payment {
            stream: ObjectId::from_hash(Hash32::from_hex(stream).map_err(|_| bad())?),
            position: position.parse().map_err(|_| bad())?,
            value: Hash32::from_hex(value).map_err(|_| bad())?,
        })
    }
}

/// Writes a jump as `<index>:<hash>,<hash>,…` (the path from the leaf up).
pub fn format_jump(jump: &Jump) -> String {
    let path: Vec<String> = jump.path.iter().map(Hash32::to_hex).collect();
    format!("{}:{}", jump.index, path.join(","))
}

/// Reads a jump written by [`format_jump`].
pub fn parse_jump(text: &str) -> Result<Jump, CliError> {
    let bad = || CliError::Usage(format!("{text:?} is not a jump (<index>:<hash>,<hash>,…)"));
    let (index, path) = text.trim().split_once(':').ok_or_else(bad)?;
    let path = match path {
        "" => Vec::new(),
        path => path
            .split(',')
            .map(|h| Hash32::from_hex(h).map_err(|_| bad()))
            .collect::<Result<_, _>>()?,
    };
    Ok(Jump {
        index: index.parse().map_err(|_| bad())?,
        path,
    })
}

/// A payment as an x402 `stream` payload carries it: the element, and the
/// jump to its checkpoint when it is the first payment past one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamPayment {
    /// The payment.
    #[serde(flatten)]
    pub payment: Payment,
    /// The jump for the checkpoint below it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jump: Option<Jump>,
}

/// The checkpoint spacing the CLI and demos use for a chain of `length`
/// steps: every 64 steps, or fewer checkpoints for chains longer than 2²²
/// steps, so the payer keeps at most 65,537 leaves.
pub fn default_spacing(length: u64) -> u64 {
    64u64.max(length.div_ceil(1 << 16)).min(length).max(1)
}

/// Reads a stream from the ledger.
pub fn load_stream(ledger: &dyn Ledger, id: &ObjectId) -> Result<Stream, CliError> {
    match ledger.object(*id)?.map(|o| o.data) {
        Some(ObjectData::Stream(stream)) => Ok(stream),
        Some(_) => Err(CliError::Usage(format!("{} is not a stream", id.short()))),
        None => Err(CliError::Node(format!("stream {} not found", id.short()))),
    }
}

/// What a payment proves against a stream's settled head.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Standing {
    /// At or below the settled position, and on the chain: paid and settled.
    Settled {
        /// Hashes the check took.
        hashes: u64,
    },
    /// Above the settled position and hashing down to the head: a genuine
    /// payment the payee can still settle.
    Unsettled {
        /// Steps above the settled position.
        steps: u64,
        /// What settling them would pay.
        value: Amount,
    },
    /// Not an element of the stream's chain.
    Forged,
}

/// Checks `payment` against `stream` as the ledger shows it: from the head
/// down for a settled position, from the payment down otherwise. Costs the
/// distance between the two positions in hashes.
pub fn standing(stream: &Stream, payment: &Payment) -> Standing {
    if payment.position > stream.length {
        return Standing::Forged;
    }
    if payment.position <= stream.position {
        let hashes = stream.position - payment.position;
        return match walk(&stream.head, hashes) == payment.value {
            true => Standing::Settled { hashes },
            false => Standing::Forged,
        };
    }
    let steps = payment.position - stream.position;
    match walk(&payment.value, steps) == stream.head {
        true => Standing::Unsettled {
            steps,
            value: Amount(stream.unit.0.saturating_mul(u128::from(steps))),
        },
        false => Standing::Forged,
    }
}

/// Like [`standing`], by a jump to the checkpoint below the payment instead
/// of a walk from the head: at most `spacing + 21` hashes, however far the
/// payment is from the settled position.
pub fn standing_by_jump(stream: &Stream, payment: &Payment, jump: &Jump) -> Standing {
    if !verify_jump(stream, payment.position, &payment.value, jump) {
        return Standing::Forged;
    }
    let hashes = jump_hashes(stream, payment.position, jump).unwrap_or(0);
    match payment.position <= stream.position {
        true => Standing::Settled { hashes },
        false => {
            let steps = payment.position - stream.position;
            Standing::Unsettled {
                steps,
                value: Amount(stream.unit.0.saturating_mul(u128::from(steps))),
            }
        }
    }
}

// ----- Payer -------------------------------------------------------------------

/// What a payer keeps for a stream it opened.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PayerRecord {
    /// The stream.
    pub stream: ObjectId,
    /// The paying wallet's name.
    pub wallet: String,
    /// Who is paid.
    pub payee: AccountId,
    /// The value of one step.
    pub unit: Amount,
    /// The chain's length.
    pub length: u64,
    /// The chain's top element: SECRET, it pays for the whole chain.
    pub seed: Hash32,
    /// The highest position revealed so far.
    pub revealed: u64,
    /// The chain's checkpoint spacing (0: no checkpoints).
    #[serde(default)]
    pub spacing: u64,
    /// The latest checkpoint whose jump was handed out with a payment.
    #[serde(default)]
    pub announced: Option<u64>,
}

/// A payer's streams: their chains and how far each has been revealed.
///
/// A book backed by a directory writes the new position to disk before it
/// hands out an element, so a crash never makes it reveal the same position
/// for two payments; an in-memory book is for demos and tests.
#[derive(Debug, Default)]
pub struct PayerBook {
    dir: Option<PathBuf>,
    open: BTreeMap<ObjectId, (PayerRecord, Option<PayerChain>)>,
}

impl PayerBook {
    /// A book kept in memory.
    pub fn in_memory() -> PayerBook {
        PayerBook::default()
    }

    /// A book kept in `dir` (one JSON file per stream).
    pub fn at(dir: PathBuf) -> PayerBook {
        PayerBook {
            dir: Some(dir),
            ..PayerBook::default()
        }
    }

    /// A fresh secret chain of `length` steps, with a checkpoint every
    /// `spacing` steps (0: none).
    pub fn new_chain(length: u64, spacing: u64) -> (Hash32, PayerChain) {
        let seed = Hash32(sion_crypto::keys::random_seed());
        (seed, Self::chain(seed, length, spacing))
    }

    fn chain(seed: Hash32, length: u64, spacing: u64) -> PayerChain {
        match spacing {
            0 => PayerChain::new(seed, length),
            spacing => PayerChain::with_checkpoints(seed, length, spacing),
        }
    }

    fn path(&self, stream: &ObjectId) -> Option<PathBuf> {
        self.dir.as_ref().map(|d| d.join(format!("{stream}.json")))
    }

    fn save(&self, record: &PayerRecord) -> Result<(), CliError> {
        let Some(path) = self.path(&record.stream) else {
            return Ok(());
        };
        let dir = path.parent().expect("a file in a directory");
        std::fs::create_dir_all(dir).map_err(|e| CliError::Usage(e.to_string()))?;
        let text = serde_json::to_string_pretty(record).expect("serializable");
        // Write and rename, so the file is never half written.
        let partial = path.with_extension("json.partial");
        std::fs::write(&partial, text).map_err(|e| CliError::Usage(e.to_string()))?;
        std::fs::rename(&partial, &path).map_err(|e| CliError::Usage(e.to_string()))
    }

    /// Adds a stream just opened with `chain`.
    pub fn add(&mut self, record: PayerRecord, chain: PayerChain) -> Result<(), CliError> {
        self.save(&record)?;
        self.open.insert(record.stream, (record, Some(chain)));
        Ok(())
    }

    /// The record of `stream`.
    pub fn record(&mut self, stream: &ObjectId) -> Result<&PayerRecord, CliError> {
        self.load(stream)?;
        Ok(&self.open[stream].0)
    }

    fn load(&mut self, stream: &ObjectId) -> Result<(), CliError> {
        if self.open.contains_key(stream) {
            return Ok(());
        }
        let path = self.path(stream).ok_or_else(|| {
            CliError::Usage(format!(
                "this session holds no chain for {}",
                stream.short()
            ))
        })?;
        let text = std::fs::read_to_string(&path).map_err(|_| {
            CliError::Usage(format!(
                "no chain for stream {} in {}",
                stream.short(),
                path.display()
            ))
        })?;
        let record: PayerRecord =
            serde_json::from_str(&text).map_err(|e| CliError::Usage(e.to_string()))?;
        self.open.insert(*stream, (record, None));
        Ok(())
    }

    /// Reveals the element `steps` positions above the last one revealed:
    /// a payment of `steps` units.
    pub fn pay(&mut self, stream: &ObjectId, steps: u64) -> Result<Payment, CliError> {
        self.load(stream)?;
        let (record, _) = &self.open[stream];
        let position = record
            .revealed
            .checked_add(steps)
            .filter(|p| *p <= record.length && steps > 0)
            .ok_or_else(|| {
                CliError::Usage(format!(
                    "stream {} has {} of {} steps left, not {steps}",
                    stream.short(),
                    record.length - record.revealed,
                    record.length
                ))
            })?;
        let mut updated = record.clone();
        updated.revealed = position;
        self.save(&updated)?;
        let (record, chain) = self.open.get_mut(stream).expect("loaded");
        *record = updated;
        let chain =
            chain.get_or_insert_with(|| Self::chain(record.seed, record.length, record.spacing));
        let value = chain.element(position).expect("position within the chain");
        Ok(Payment {
            stream: *stream,
            position,
            value,
        })
    }

    /// The jump for checkpoint `index` of `stream`'s chain, if it has
    /// checkpoints.
    pub fn jump(&mut self, stream: &ObjectId, index: u64) -> Result<Option<Jump>, CliError> {
        self.load(stream)?;
        let (record, chain) = self.open.get_mut(stream).expect("loaded");
        if record.spacing == 0 {
            return Ok(None);
        }
        let chain =
            chain.get_or_insert_with(|| Self::chain(record.seed, record.length, record.spacing));
        Ok(chain.jump(index))
    }

    /// Like [`PayerBook::pay`], attaching the jump for the payment's
    /// checkpoint the first time a payment reaches it (never for checkpoint
    /// 0: walking down to the anchor is always cheaper).
    pub fn pay_with_jump(
        &mut self,
        stream: &ObjectId,
        steps: u64,
    ) -> Result<StreamPayment, CliError> {
        let payment = self.pay(stream, steps)?;
        let record = &self.open[stream].0;
        let index = payment.position.checked_div(record.spacing).unwrap_or(0);
        if index == 0 || record.announced == Some(index) {
            return Ok(StreamPayment {
                payment,
                jump: None,
            });
        }
        let jump = self.jump(stream, index)?;
        let mut updated = self.open[stream].0.clone();
        updated.announced = Some(index);
        self.save(&updated)?;
        self.open.get_mut(stream).expect("loaded").0 = updated;
        Ok(StreamPayment { payment, jump })
    }

    /// Goes back to `position` after a payee refused the payments above it:
    /// the next payment re-reveals elements it has already seen, so nothing
    /// new is given away.
    pub fn resync(&mut self, stream: &ObjectId, position: u64) -> Result<(), CliError> {
        self.load(stream)?;
        let (record, _) = &self.open[stream];
        if position > record.revealed {
            return Err(CliError::Protocol(format!(
                "the payee claims position {position} of stream {}, beyond the {} revealed",
                stream.short(),
                record.revealed
            )));
        }
        let mut updated = record.clone();
        updated.revealed = position;
        // The payee may have missed the jump too: attach it again.
        updated.announced = None;
        self.save(&updated)?;
        self.open.get_mut(stream).expect("loaded").0 = updated;
        Ok(())
    }
}

// ----- Payee -------------------------------------------------------------------

/// A settlement sent and not yet executed.
#[derive(Clone, Debug)]
struct Pending {
    digest: TxDigest,
    fee: Amount,
    /// The hashes it costs the validators to check.
    hashes: u64,
}

/// The payee's view of one stream.
#[derive(Clone, Debug)]
struct Tab {
    /// The stream as the ledger last showed it.
    terms: Stream,
    /// When the terms were last read.
    read: Instant,
    /// The latest payment accepted, and its element.
    position: u64,
    value: Hash32,
    /// When the oldest unsettled payment was accepted.
    since: Option<Instant>,
    pending: Option<Pending>,
    /// The latest checkpoint jump the payer handed over, checked.
    jump: Option<Jump>,
}

impl Tab {
    fn unsettled(&self) -> u64 {
        self.position - self.terms.position
    }
}

/// What a payee's desk has done.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct DeskReport {
    /// Payments accepted.
    pub payments: u64,
    /// Payments refused.
    pub refused: u64,
    /// Hashes checked to accept them.
    pub hashes: u64,
    /// Settlements executed.
    pub settlements: u64,
    /// Steps those settlements covered.
    pub settled_steps: u64,
    /// What they paid in.
    pub settled_value: Amount,
    /// What they cost in fees.
    pub fees: Amount,
    /// The hashes the validators checked for them.
    pub ledger_hashes: u64,
    /// How many of them were proven by a jump to a checkpoint.
    pub jumps: u64,
}

/// A payment the desk accepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Accepted {
    /// Who pays.
    pub payer: AccountId,
    /// The stream's position after this payment.
    pub position: u64,
    /// The position the ledger has settled.
    pub settled: u64,
}

/// A refused payment, and the position the desk had accepted up to (so an
/// honest payer can resynchronize).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refused {
    /// Why.
    pub reason: String,
    /// The desk's position for the stream, if it knows the stream.
    pub position: Option<u64>,
}

/// The payee's side of payment streams: accepts payments with one hash per
/// step, and settles them in batches.
pub struct StreamDesk {
    wallet: Mutex<Wallet>,
    payee: AccountId,
    /// Settle once a stream has this many unsettled steps.
    pub settle_every: u64,
    /// Settle a stream whose oldest unsettled payment is this old.
    pub settle_after: Duration,
    /// Refuse payments from a stream that ends within this many Pulses, and
    /// settle everything from it once it is twice as close.
    pub margin: u64,
    tabs: Mutex<BTreeMap<ObjectId, Tab>>,
    report: Mutex<DeskReport>,
    /// Why the last settlement could not be sent or did not succeed.
    trouble: Mutex<Option<String>>,
}

impl fmt::Debug for StreamDesk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "StreamDesk({})", self.payee.short())
    }
}

/// How long the desk trusts the terms it read before reading them again
/// when a payment needs more escrow or time than they show.
const REREAD: Duration = Duration::from_millis(500);

impl StreamDesk {
    /// A desk that settles as `wallet`'s account, the payee.
    pub fn new(wallet: Wallet) -> StreamDesk {
        StreamDesk {
            payee: wallet.id,
            wallet: Mutex::new(wallet),
            settle_every: 1_000,
            settle_after: Duration::from_secs(30),
            margin: 10,
            tabs: Mutex::new(BTreeMap::new()),
            report: Mutex::new(DeskReport::default()),
            trouble: Mutex::new(None),
        }
    }

    /// The account paid.
    pub fn payee(&self) -> AccountId {
        self.payee
    }

    /// What the desk has done so far.
    pub fn report(&self) -> DeskReport {
        self.report.lock().expect("report lock").clone()
    }

    /// Accepts `payment` as the price of one request, at Pulse `now`, or
    /// says why not. Checking costs `price / unit` hashes, and one read of
    /// the ledger the first time the desk sees a stream (or when a payment
    /// needs more escrow or time than the terms it read show). A `jump` for
    /// the payment's checkpoint is checked and kept for settling; a jump
    /// that does not check is ignored (the desk can always walk).
    pub fn accept(
        &self,
        ledger: &dyn Ledger,
        now: Pulse,
        payment: &Payment,
        price: Amount,
        jump: Option<&Jump>,
    ) -> Result<Accepted, Refused> {
        let result = self.check(ledger, now, payment, price, jump);
        let mut report = self.report.lock().expect("report lock");
        match &result {
            Ok(_) => report.payments += 1,
            Err(_) => report.refused += 1,
        }
        result
    }

    fn check(
        &self,
        ledger: &dyn Ledger,
        now: Pulse,
        payment: &Payment,
        price: Amount,
        jump: Option<&Jump>,
    ) -> Result<Accepted, Refused> {
        let refuse = |reason: String, position: Option<u64>| Refused { reason, position };
        let mut tabs = self.tabs.lock().expect("tabs lock");
        let tab = match tabs.entry(payment.stream) {
            std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::btree_map::Entry::Vacant(entry) => {
                let terms = load_stream(ledger, &payment.stream)
                    .map_err(|e| refuse(e.to_string(), None))?;
                if terms.payee != self.payee {
                    return Err(refuse("the stream does not pay this server".into(), None));
                }
                if terms.asset != AssetId::usd() {
                    return Err(refuse("the stream is not in USD".into(), None));
                }
                entry.insert(Tab {
                    position: terms.position,
                    value: terms.head,
                    terms,
                    read: Instant::now(),
                    since: None,
                    pending: None,
                    jump: None,
                })
            }
        };
        let here = Some(tab.position);
        if price == Amount::ZERO || price.0 % tab.terms.unit.0 != 0 {
            return Err(refuse(
                format!(
                    "the price, {price} USD, is not a whole number of the stream's {} USD steps",
                    tab.terms.unit
                ),
                here,
            ));
        }
        let steps = u64::try_from(price.0 / tab.terms.unit.0).unwrap_or(u64::MAX);
        let expected = tab.position.saturating_add(steps);
        if payment.position != expected {
            return Err(refuse(
                format!(
                    "expected the element at position {expected}, not {}",
                    payment.position
                ),
                here,
            ));
        }
        if payment.position > tab.terms.length {
            return Err(refuse("the stream's chain is used up".into(), here));
        }
        // One hash per step: the element must hash down to the last one.
        if walk(&payment.value, steps) != tab.value {
            return Err(refuse(
                "the payment does not hash to the last one accepted".into(),
                here,
            ));
        }
        let covered = |tab: &Tab| {
            let owed = tab
                .terms
                .unit
                .0
                .saturating_mul(u128::from(payment.position - tab.terms.position));
            owed <= tab.terms.escrow.0 && now.0.saturating_add(self.margin) <= tab.terms.until.0
        };
        if !covered(tab) && tab.read.elapsed() > REREAD {
            // The payer may have topped the stream up since the desk read it.
            if let Ok(terms) = load_stream(ledger, &payment.stream) {
                if terms.position <= tab.position {
                    tab.terms = terms;
                    tab.read = Instant::now();
                }
            }
        }
        if !tab.terms.open {
            return Err(refuse("the stream is closed".into(), here));
        }
        if !covered(tab) {
            return Err(refuse(
                format!(
                    "the stream's escrow or time does not cover this payment (it ends at Pulse {}, this server needs {} Pulses to settle); top it up",
                    tab.terms.until, self.margin
                ),
                here,
            ));
        }
        tab.position = payment.position;
        tab.value = payment.value;
        tab.since.get_or_insert_with(Instant::now);
        let mut hashes = steps;
        if let Some(jump) = jump {
            if verify_jump(&tab.terms, payment.position, &payment.value, jump) {
                hashes += jump_hashes(&tab.terms, payment.position, jump).unwrap_or(0);
                tab.jump = Some(jump.clone());
            }
        }
        self.report.lock().expect("report lock").hashes += hashes;
        Ok(Accepted {
            payer: tab.terms.payer,
            position: tab.position,
            settled: tab.terms.position,
        })
    }

    /// Checks the settlements in flight; returns how many still are.
    pub fn refresh(&self, ledger: &dyn Ledger) -> Result<usize, CliError> {
        let mut tabs = self.tabs.lock().expect("tabs lock");
        let mut in_flight = 0;
        for (id, tab) in tabs.iter_mut() {
            let Some(pending) = tab.pending.clone() else {
                continue;
            };
            match ledger.tx(pending.digest)? {
                TxState::Executed(view) => {
                    tab.pending = None;
                    let before = tab.terms.position;
                    tab.terms = load_stream(ledger, id)?;
                    tab.read = Instant::now();
                    if tab.terms.position > tab.position {
                        // Settled by another desk for the same payee.
                        tab.position = tab.terms.position;
                        tab.value = tab.terms.head;
                    }
                    if tab.unsettled() == 0 {
                        tab.since = None;
                    }
                    if view.status != ExecStatus::Success {
                        *self.trouble.lock().expect("trouble lock") =
                            Some(format!("a settlement did not succeed: {:?}", view.status));
                    }
                    if view.status == ExecStatus::Success {
                        let steps = tab.terms.position.saturating_sub(before);
                        let mut report = self.report.lock().expect("report lock");
                        report.settlements += 1;
                        report.settled_steps += steps;
                        report.settled_value =
                            Amount(report.settled_value.0 + tab.terms.unit.0 * u128::from(steps));
                        report.fees = Amount(report.fees.0 + pending.fee.0);
                        report.ledger_hashes += pending.hashes;
                        if pending.hashes != steps {
                            report.jumps += 1;
                        }
                    }
                }
                TxState::Rejected(reason) => {
                    *self.trouble.lock().expect("trouble lock") =
                        Some(format!("a settlement was not certified: {reason}"));
                    tab.pending = None;
                }
                TxState::Pending(_) | TxState::Unknown => in_flight += 1,
            }
        }
        if in_flight == 0 {
            // No coin of the payee's is committed any more.
            self.wallet.lock().expect("wallet lock").settle();
        }
        Ok(in_flight)
    }

    /// Sends the settlements that are due (with `all`, one for every stream
    /// with unsettled steps); returns how many it sent. A stream has at most
    /// one settlement in flight, of at most the steps one may check.
    pub fn settle(
        &self,
        ledger: &dyn Ledger,
        config: &ChainConfig,
        now: Pulse,
        all: bool,
    ) -> Result<usize, CliError> {
        let mut tabs = self.tabs.lock().expect("tabs lock");
        let mut wallet = self.wallet.lock().expect("wallet lock");
        let view = RemoteView::new(ledger, config.clone());
        let mut sent = 0;
        for (id, tab) in tabs.iter_mut() {
            let unsettled = tab.unsettled();
            if tab.pending.is_some() || unsettled == 0 || !tab.terms.open {
                continue;
            }
            let ending = now.0.saturating_add(2 * self.margin) >= tab.terms.until.0;
            let old = tab.since.is_some_and(|t| t.elapsed() >= self.settle_after);
            if !(all || ending || old || unsettled >= self.settle_every) {
                continue;
            }
            let (position, jump) = plan(tab);
            let value = walk(&tab.value, tab.position - position);
            let hashes = match &jump {
                Some(jump) => jump_hashes(&tab.terms, position, jump).unwrap_or(0),
                None => position - tab.terms.position,
            };
            let kind = TxKind::SettleStream {
                stream: *id,
                position,
                value,
                jump,
            };
            let signed = match wallet.build(&view, kind, Vec::new()) {
                Ok(signed) => signed,
                // No free coin for the fee yet: try again next time.
                Err(error) => {
                    *self.trouble.lock().expect("trouble lock") =
                        Some(format!("cannot build a settlement: {error}"));
                    continue;
                }
            };
            ledger.submit(&signed)?;
            tab.pending = Some(Pending {
                digest: signed.digest(),
                fee: signed.tx.fee,
                hashes,
            });
            sent += 1;
        }
        Ok(sent)
    }

    /// Settles every unsettled step and waits until the settlements have
    /// executed (or `timeout` passes).
    pub fn close_out(
        &self,
        ledger: &dyn Ledger,
        config: &ChainConfig,
        timeout: Duration,
    ) -> Result<(), CliError> {
        let deadline = Instant::now() + timeout;
        loop {
            let in_flight = self.refresh(ledger)?;
            let unsettled: u64 = self
                .tabs
                .lock()
                .expect("tabs lock")
                .values()
                .filter(|t| t.terms.open)
                .map(Tab::unsettled)
                .sum();
            if in_flight == 0 && unsettled == 0 {
                return Ok(());
            }
            if Instant::now() > deadline {
                return Err(CliError::Transaction(format!(
                    "{unsettled} stream steps still unsettled{}",
                    self.trouble
                        .lock()
                        .expect("trouble lock")
                        .as_ref()
                        .map(|t| format!(" ({t})"))
                        .unwrap_or_default()
                )));
            }
            self.settle(ledger, config, ledger.pulse()?, true)?;
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// The latest payment the desk accepted from `stream`, and the position
    /// the ledger has settled.
    pub fn position(&self, stream: &ObjectId) -> Option<(u64, u64)> {
        self.tabs
            .lock()
            .expect("tabs lock")
            .get(stream)
            .map(|t| (t.position, t.terms.position))
    }
}

/// Where to settle a tab to, and how: walking from the head (at most
/// [`MAX_SETTLE_STEPS`] steps), or jumping to the checkpoint the payer
/// handed over, whichever covers more steps (the cheaper one on a tie).
fn plan(tab: &Tab) -> (u64, Option<Jump>) {
    let from = tab.terms.position;
    let walked = from + tab.unsettled().min(MAX_SETTLE_STEPS);
    let Some(jump) = &tab.jump else {
        return (walked, None);
    };
    let base = jump.index.saturating_mul(tab.terms.spacing);
    let top = tab
        .position
        .min(base.saturating_add(tab.terms.spacing.saturating_sub(1)));
    if top <= from || top < base {
        return (walked, None);
    }
    let jump_cost = jump_hashes(&tab.terms, top, jump).unwrap_or(u64::MAX);
    if top > walked || (top == walked && jump_cost < walked - from) {
        (top, Some(jump.clone()))
    } else {
        (walked, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payments_round_trip_as_text() {
        let payment = Payment {
            stream: ObjectId::derive(b"s"),
            position: 42,
            value: Hash32([9; 32]),
        };
        let text = payment.to_string();
        assert_eq!(text.parse::<Payment>().unwrap(), payment);
        assert!("nope".parse::<Payment>().is_err());
        assert!(format!("{text}:extra").parse::<Payment>().is_err());
    }

    #[test]
    fn a_book_never_reveals_a_position_twice_and_survives_a_restart() {
        let dir = std::env::temp_dir().join(format!(
            "sion-stream-book-{}",
            sion_core::hash::Hash32(sion_crypto::keys::random_seed()).short()
        ));
        let (seed, chain) = PayerBook::new_chain(50, 0);
        let stream = ObjectId::derive(b"stream");
        let record = PayerRecord {
            stream,
            wallet: "agent".into(),
            payee: AccountId::derive(b"api"),
            unit: Amount(1_000),
            length: 50,
            seed,
            revealed: 0,
            spacing: 0,
            announced: None,
        };
        let anchor = chain.anchor();
        let mut book = PayerBook::at(dir.clone());
        book.add(record, chain).unwrap();
        let first = book.pay(&stream, 1).unwrap();
        let second = book.pay(&stream, 3).unwrap();
        assert_eq!((first.position, second.position), (1, 4));
        assert_eq!(walk(&second.value, 3), first.value);
        assert_eq!(walk(&first.value, 1), anchor);
        // A new process picks up where this one stopped.
        let mut again = PayerBook::at(dir.clone());
        let third = again.pay(&stream, 1).unwrap();
        assert_eq!(third.position, 5);
        assert_eq!(walk(&third.value, 1), second.value);
        assert!(again.pay(&stream, 46).is_err(), "only 45 steps are left");
        // Resynchronizing re-reveals an element already given out.
        again.resync(&stream, 4).unwrap();
        assert_eq!(again.pay(&stream, 1).unwrap(), third);
        assert!(again.resync(&stream, 9).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_desk_settles_by_jump_when_that_covers_more() {
        let mut chain = PayerChain::with_checkpoints(Hash32([5; 32]), 100_000, 100);
        let checkpoints = chain.checkpoints().unwrap();
        let terms = Stream {
            payer: AccountId::derive(b"payer"),
            payee: AccountId::derive(b"payee"),
            asset: AssetId::usd(),
            unit: Amount(1),
            length: 100_000,
            position: 1_000,
            head: chain.element(1_000).unwrap(),
            escrow: Amount(100_000),
            until: Pulse(500),
            open: true,
            spacing: 100,
            checkpoints: checkpoints.root,
        };
        let tab = |position: u64, jump: Option<Jump>, chain: &mut PayerChain| Tab {
            terms: terms.clone(),
            read: Instant::now(),
            position,
            value: chain.element(position).unwrap(),
            since: None,
            pending: None,
            jump,
        };
        // Without a jump: walk, at most 65,536 steps.
        assert_eq!(plan(&tab(1_050, None, &mut chain)), (1_050, None));
        assert_eq!(plan(&tab(90_000, None, &mut chain)), (66_536, None));
        // A jump to checkpoint 899 covers all 88,950 steps up to 89,950.
        let j899 = chain.jump(899).unwrap();
        let (position, jump) = plan(&tab(89_950, Some(j899.clone()), &mut chain));
        assert_eq!((position, jump.as_ref()), (89_950, Some(&j899)));
        // A stale jump covers its own segment only: from checkpoint 899 the
        // top is 89,999, more than a walk covers.
        let (position, jump) = plan(&tab(90_500, Some(j899.clone()), &mut chain));
        assert_eq!((position, jump.is_some()), (89_999, true));
        // For a few steps a walk is cheaper than the jump's path.
        let j10 = chain.jump(10).unwrap();
        assert_eq!(plan(&tab(1_003, Some(j10), &mut chain)), (1_003, None));
    }

    #[test]
    fn standing_tells_settled_unsettled_and_forged_apart() {
        let mut chain = PayerChain::new(Hash32([4; 32]), 100);
        let stream = Stream {
            payer: AccountId::derive(b"payer"),
            payee: AccountId::derive(b"payee"),
            asset: AssetId::usd(),
            unit: Amount(1_000),
            length: 100,
            position: 40,
            head: chain.element(40).unwrap(),
            escrow: Amount(60_000),
            until: Pulse(500),
            open: true,
            spacing: 0,
            checkpoints: Hash32::ZERO,
        };
        let id = ObjectId::derive(b"stream");
        let at = |chain: &mut PayerChain, position| Payment {
            stream: id,
            position,
            value: chain.element(position).unwrap(),
        };
        assert_eq!(
            standing(&stream, &at(&mut chain, 25)),
            Standing::Settled { hashes: 15 }
        );
        assert_eq!(
            standing(&stream, &at(&mut chain, 52)),
            Standing::Unsettled {
                steps: 12,
                value: Amount(12_000)
            }
        );
        let forged = Payment {
            value: Hash32([1; 32]),
            ..at(&mut chain, 52)
        };
        assert_eq!(standing(&stream, &forged), Standing::Forged);
        let beyond = Payment {
            position: 101,
            ..forged
        };
        assert_eq!(standing(&stream, &beyond), Standing::Forged);
    }
}
