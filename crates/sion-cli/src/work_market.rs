//! A market for agents' work, priced per accepted result.
//!
//! A buyer of an agent's work usually takes the seller's price or haggles
//! alone, and nobody else learns what the work went for. Here work of one
//! kind is traded in rounds, each cleared at one price that anyone can
//! recompute:
//!
//! 1. **A market** is a claim ([`Market`]) that fixes what one unit of
//!    work is, the rule a result must pass to be paid (a Task Charter's
//!    acceptance), a price band, and the rhythm of its rounds: so many
//!    Pulses to bid, then so many to reveal.
//! 2. **Bids are sealed.** A bid ([`SealedBid`]) says which side it is on
//!    and commits to its quantity and price. With it goes a zero-knowledge
//!    proof that the price is inside the market's band and the quantity
//!    within its cap, so that a sealed bid is known to be a bid before
//!    anyone opens it.
//! 3. **Bids are revealed** in the round's second window ([`Revealed`]). A
//!    bid whose reveal is missing or does not match what it committed to
//!    is left out.
//! 4. **The round clears at one price.** The revealed bids are a batch for
//!    the same certified double auction the ledger's markets clear with
//!    (`sion-clearing`): the allocation that maximises the gains from
//!    trade, ties broken by the beacon of the Pulse the round closed in,
//!    one price for every unit. Nobody is trusted to compute it: it is a
//!    function of what the ledger holds ([`cleared`]).
//! 5. **Buyers post tasks.** For each unit, the buyer posts a Task Charter
//!    to the worker it was paired with, at the round's price, under the
//!    market's acceptance. The money is escrowed then, and a worker is
//!    paid only for a result that is accepted.
//!
//! A bid is a signed promise with nothing escrowed behind it. Who posted
//! the tasks they owed, who delivered and who never revealed is read from
//! the ledger ([`followed`]).

use std::collections::BTreeMap;
use std::path::PathBuf;

use clap::{Subcommand, ValueEnum};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sion_clearing::{Batch, ClearingResult, Order, Side};
use sion_core::hash::Hash32;
use sion_core::ids::{AccountId, AtomId, ClaimId, DomainId, ObjectId, Pulse};
use sion_core::Amount;
use sion_exec::claim::{formulas, ClaimBody, Payload};
use sion_exec::execute::Event;
use sion_exec::task::Acceptance;
use sion_exec::TxKind;
use sion_zk::{Commitment, Opening, Proof, Statement};

use crate::error::CliError;
use crate::feeds::{self, Feed};
use crate::ledger::{Ledger, LedgerExt};
use crate::session::Session;

/// The schema of a claim opening a market.
pub const MARKET_SCHEMA: &str = "work.market/v1";

/// The schema of a sealed bid.
pub const BID_SCHEMA: &str = "work.bid/v1";

/// The schema of a bid's reveal.
pub const REVEAL_SCHEMA: &str = "work.reveal/v1";

/// The schema of a claim recording what a round cleared at.
pub const CLEARED_SCHEMA: &str = "work.cleared/v1";

/// The most units a bid may be for, whatever a market allows.
pub const MAX_QUANTITY: u64 = 10_000;

/// A market for one kind of work.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Market {
    /// Its name: lowercase letters, digits and hyphens.
    pub name: String,
    /// What one unit of work is, in words a worker can price.
    pub unit: String,
    /// The rule a result must pass to be paid: every task posted from this
    /// market is posted under it.
    pub acceptance: Acceptance,
    /// The first Pulse of its first round.
    pub first: u64,
    /// How many Pulses each round takes bids for.
    pub bidding: u64,
    /// How many Pulses, after those, it takes reveals for.
    pub revealing: u64,
    /// The lowest price a bid may name, in millionths of a USD.
    pub min_price: u64,
    /// The highest.
    pub max_price: u64,
    /// The most units one bid may be for.
    pub max_quantity: u64,
    /// How many Pulses after a round closes its buyers have to post their
    /// tasks.
    pub post_within: u64,
    /// How many Pulses a posted task's worker has to settle it.
    pub deadline: u64,
}

impl Market {
    /// The Pulses a round takes, bidding and revealing.
    pub fn span(&self) -> u64 {
        self.bidding + self.revealing
    }

    /// The round Pulse `pulse` is in, if the market has opened by then.
    pub fn round_at(&self, pulse: u64) -> Option<u64> {
        pulse
            .checked_sub(self.first)
            .map(|since| since / self.span())
    }

    /// The first Pulse bids of `round` are taken in.
    pub fn bids_from(&self, round: u64) -> u64 {
        self.first + round * self.span()
    }

    /// The first Pulse reveals of `round` are taken in: bids are taken
    /// before it.
    pub fn reveals_from(&self, round: u64) -> u64 {
        self.bids_from(round) + self.bidding
    }

    /// The Pulse `round` closes in: reveals are taken before it, and its
    /// beacon breaks the round's ties.
    pub fn closes(&self, round: u64) -> u64 {
        self.bids_from(round) + self.span()
    }

    /// What it says of itself that cannot hold, if anything.
    pub fn check(&self) -> Result<(), String> {
        let named = !self.name.is_empty()
            && self.name.len() <= 64
            && self
                .name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if !named {
            return Err(format!(
                "{:?} is not a market's name: lowercase letters, digits and hyphens",
                self.name
            ));
        }
        if self.unit.trim().is_empty() || self.unit.len() > 2_000 {
            return Err("a market says what one unit of work is, in at most 2000 bytes".into());
        }
        if self.bidding == 0 || self.revealing == 0 {
            return Err("a round takes bids and reveals for at least a Pulse each".into());
        }
        if self.min_price == 0 || self.min_price > self.max_price {
            return Err("a market's lowest price is above zero and at most its highest".into());
        }
        if self.max_price > i64::MAX as u64 / MAX_QUANTITY {
            return Err("the highest price is more than a market can clear".into());
        }
        if self.max_quantity == 0 || self.max_quantity > MAX_QUANTITY {
            return Err(format!(
                "a bid is for 1 to {MAX_QUANTITY} units: the market's cap is within that"
            ));
        }
        if self.deadline == 0 {
            return Err("a posted task's worker has at least a Pulse to settle it".into());
        }
        Ok(())
    }
}

/// A sealed bid, as its claim holds it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SealedBid {
    /// The market's claim.
    pub market: String,
    /// The round.
    pub round: u64,
    /// Whether it buys work or offers it.
    pub side: Side,
    /// What binds the bid: the digest of its quantity, price and a salt,
    /// with the market, the round, the side and the bidder.
    pub commitment: String,
    /// A commitment to its price, in millionths of a USD.
    pub price: Commitment,
    /// A commitment to its quantity.
    pub quantity: Commitment,
    /// Proof that the price is inside the market's band and the quantity
    /// at least one and within its cap.
    pub proof: Proof,
}

/// What a bidder keeps to itself until it reveals.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Secret {
    /// The market's claim.
    pub market: String,
    /// The round.
    pub round: u64,
    /// The side.
    pub side: Side,
    /// The salt, in hex.
    pub salt: String,
    /// The price and what hides it.
    pub price: Opening,
    /// The quantity and what hides it.
    pub quantity: Opening,
    /// The bid's claim, once published.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim: Option<String>,
}

/// A bid's reveal, as its claim holds it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Revealed {
    /// The bid's claim.
    pub bid: String,
    /// The salt, in hex.
    pub salt: String,
    /// The price and what hid it.
    pub price: Opening,
    /// The quantity and what hid it.
    pub quantity: Opening,
}

/// What a round cleared at, as a claim records it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recorded {
    /// The market's claim.
    pub market: String,
    /// The round.
    pub round: u64,
    /// The price of a unit, in millionths of a USD: none where nothing
    /// traded.
    pub price: Option<u64>,
    /// The units traded.
    pub volume: u64,
    /// The digest of the batch that was cleared.
    pub batch: String,
}

fn short(claim: &ClaimId) -> String {
    feeds::short_key(claim.as_hash())
}

fn market_key(name: &str) -> String {
    format!("work-market:{name}")
}

fn bid_key(market: &ClaimId, round: u64) -> String {
    format!("work-bid:{}:{round}", short(market))
}

fn reveal_key(market: &ClaimId, round: u64) -> String {
    format!("work-reveal:{}:{round}", short(market))
}

fn cleared_key(market: &ClaimId, round: u64) -> String {
    format!("work-cleared:{}:{round}", short(market))
}

fn side_word(side: Side) -> &'static str {
    match side {
        Side::Buy => "buy",
        Side::Sell => "sell",
    }
}

/// What binds a bid: nothing but hashing stands behind it, so it holds
/// whatever becomes of the proof beside it.
fn commitment(
    market: &ClaimId,
    round: u64,
    bidder: &AccountId,
    side: Side,
    quantity: u64,
    price: u64,
    salt: &str,
) -> String {
    let said = serde_json::json!({
        "market": market.as_hash().to_hex(),
        "round": round,
        "bidder": bidder.as_hash().to_hex(),
        "side": side_word(side),
        "quantity": quantity,
        "price": price,
        "salt": salt,
    });
    sion_core::hash::hash_domain("sion-cli/work-market/bid/v1", said.to_string().as_bytes())
        .to_hex()
}

/// What a bid's proof is bound to: this market, round, bidder and bid.
fn context(market: &ClaimId, round: u64, bidder: &AccountId, commitment: &str) -> Vec<u8> {
    format!(
        "sion-cli/work-market/proof/v1|{}|{round}|{}|{commitment}",
        market.as_hash().to_hex(),
        bidder.as_hash().to_hex()
    )
    .into_bytes()
}

/// What a bid's proof shows of its price (the first number) and quantity
/// (the second).
fn statements(market: &Market) -> Vec<Statement> {
    vec![
        Statement::at_least(0, market.min_price),
        Statement::at_most(0, market.max_price),
        Statement::at_least(1, 1),
        Statement::at_most(1, market.max_quantity),
    ]
}

/// Seals a bid of `bidder`'s in `round`: `quantity` units at `price` a
/// unit. Returns the bid to publish and the secret to keep.
pub fn seal(
    claim: &ClaimId,
    market: &Market,
    round: u64,
    bidder: &AccountId,
    side: Side,
    quantity: u64,
    price: Amount,
) -> Result<(SealedBid, Secret), CliError> {
    let price = u64::try_from(price.micros())
        .map_err(|_| CliError::Usage("that price is more than a bid can name".into()))?;
    if price < market.min_price || price > market.max_price {
        return Err(CliError::Usage(format!(
            "this market takes prices from {} to {} USD a unit",
            Amount(u128::from(market.min_price)),
            Amount(u128::from(market.max_price))
        )));
    }
    if quantity == 0 || quantity > market.max_quantity {
        return Err(CliError::Usage(format!(
            "a bid in this market is for 1 to {} units",
            market.max_quantity
        )));
    }
    let mut salt = [0u8; 32];
    OsRng.fill_bytes(&mut salt);
    let salt = hex::encode(salt);
    let commitment = commitment(claim, round, bidder, side, quantity, price, &salt);
    let (price_commitment, price_opening) = sion_zk::commit(price);
    let (quantity_commitment, quantity_opening) = sion_zk::commit(quantity);
    let proof = sion_zk::prove(
        &context(claim, round, bidder, &commitment),
        &[price_opening.clone(), quantity_opening.clone()],
        &statements(market),
    )
    .map_err(|e| CliError::Usage(format!("the bid's proof could not be made: {e}")))?;
    let hex = claim.as_hash().to_hex();
    Ok((
        SealedBid {
            market: hex.clone(),
            round,
            side,
            commitment,
            price: price_commitment,
            quantity: quantity_commitment,
            proof,
        },
        Secret {
            market: hex,
            round,
            side,
            salt,
            price: price_opening,
            quantity: quantity_opening,
            claim: None,
        },
    ))
}

/// Whether `sealed`, a bid of `bidder`'s, is proven to be inside the
/// market's band and cap.
pub fn proven(claim: &ClaimId, market: &Market, bidder: &AccountId, sealed: &SealedBid) -> bool {
    sion_zk::verify(
        &context(claim, sealed.round, bidder, &sealed.commitment),
        &[sealed.price, sealed.quantity],
        &statements(market),
        &sealed.proof,
    )
    .is_ok()
}

/// The quantity and price `revealed` opens `sealed` to, if it is the
/// opening of that bid of `bidder`'s: an error says what does not match.
pub fn opened(
    claim: &ClaimId,
    market: &Market,
    bidder: &AccountId,
    sealed: &SealedBid,
    revealed: &Revealed,
) -> Result<(u64, u64), String> {
    let (quantity, price) = (revealed.quantity.value, revealed.price.value);
    let again = commitment(
        claim,
        sealed.round,
        bidder,
        sealed.side,
        quantity,
        price,
        &revealed.salt,
    );
    if again != sealed.commitment {
        return Err("what it reveals is not what the bid committed to".into());
    }
    if revealed.price.commitment() != sealed.price
        || revealed.quantity.commitment() != sealed.quantity
    {
        return Err("what it reveals is not what the bid's proof was about".into());
    }
    // Checked again in the clear: the round does not rest on the proof.
    if price < market.min_price || price > market.max_price {
        return Err("its price is outside the market's band".into());
    }
    if quantity == 0 || quantity > market.max_quantity {
        return Err("its quantity is outside the market's cap".into());
    }
    Ok((quantity, price))
}

/// Where a sealed bid stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Standing {
    /// Revealed, and what it opened to: its quantity and its price.
    Open {
        /// Units.
        quantity: u64,
        /// The price of a unit, in millionths of a USD.
        price: u64,
    },
    /// Sealed still: no reveal that matches it was published in time.
    Sealed,
    /// Left out, and why.
    Out(String),
}

/// A bid found on the ledger.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bid {
    /// Its claim.
    pub claim: ClaimId,
    /// Who made it.
    pub bidder: AccountId,
    /// The Pulse it was issued in.
    pub at: u64,
    /// Its side.
    pub side: Side,
    /// Where it stands.
    pub standing: Standing,
}

/// The bids of `round` of the market `claim`, oldest first: each sealed
/// bid issued in the bidding window and proven inside the market's band,
/// one for an account and a side, with the reveal its bidder published in
/// the reveal window if there is one that matches.
pub fn bids(
    ledger: &dyn Ledger,
    claim: ClaimId,
    market: &Market,
    round: u64,
) -> Result<Vec<Bid>, CliError> {
    let hex = claim.as_hash().to_hex();
    // A round read in part would clear at another price.
    feeds::need_since(
        ledger,
        market.bids_from(round),
        &format!("round {round}'s bids"),
    )?;
    let mut feed = Feed {
        next: market.bids_from(round),
        ..Feed::default()
    };
    let (bid_key, reveal_key) = (bid_key(&claim, round), reveal_key(&claim, round));
    feed.scan(ledger, &[&bid_key, &reveal_key])?;
    let mut found: Vec<(Bid, Option<SealedBid>)> = Vec::new();
    for entry in feed.entries.iter().filter(|e| e.key == bid_key) {
        let Some(read) = feeds::read::<SealedBid>(ledger, entry.claim, BID_SCHEMA)? else {
            continue;
        };
        if read.issuer != entry.owner
            || read.says.market != hex
            || read.says.round != round
            || found.iter().any(|(had, _)| had.claim == entry.claim)
        {
            continue;
        }
        let standing = if read.issued_at < market.bids_from(round)
            || read.issued_at >= market.reveals_from(round)
        {
            Standing::Out("it was not made while the round took bids".into())
        } else if !proven(&claim, market, &read.issuer, &read.says) {
            Standing::Out("its proof does not show a bid inside the market's band".into())
        } else if found.iter().any(|(had, _)| {
            had.bidder == read.issuer
                && had.side == read.says.side
                && !matches!(had.standing, Standing::Out(_))
        }) {
            Standing::Out("its account already has a bid on this side of the round".into())
        } else {
            Standing::Sealed
        };
        found.push((
            Bid {
                claim: entry.claim,
                bidder: read.issuer,
                at: read.issued_at,
                side: read.says.side,
                standing,
            },
            Some(read.says),
        ));
    }
    for entry in feed.entries.iter().filter(|e| e.key == reveal_key) {
        let Some(read) = feeds::read::<Revealed>(ledger, entry.claim, REVEAL_SCHEMA)? else {
            continue;
        };
        if read.issuer != entry.owner
            || read.issued_at < market.reveals_from(round)
            || read.issued_at >= market.closes(round)
        {
            continue;
        }
        let Some((bid, Some(sealed))) = found.iter_mut().find(|(bid, _)| {
            bid.claim.as_hash().to_hex() == read.says.bid && bid.bidder == read.issuer
        }) else {
            continue;
        };
        if bid.standing != Standing::Sealed {
            continue;
        }
        bid.standing = match opened(&claim, market, &read.issuer, sealed, &read.says) {
            Ok((quantity, price)) => Standing::Open { quantity, price },
            Err(why) => Standing::Out(why),
        };
    }
    let mut bids: Vec<Bid> = found.into_iter().map(|(bid, _)| bid).collect();
    bids.sort_by_key(|bid| (bid.at, *bid.claim.as_hash()));
    Ok(bids)
}

/// One buyer's units with one worker, from a round's clearing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pair {
    /// Who buys.
    pub buyer: AccountId,
    /// Who works.
    pub worker: AccountId,
    /// How many units.
    pub units: u64,
}

/// A round, cleared.
#[derive(Clone, Debug)]
pub struct Cleared {
    /// The bids that were revealed, as the batch that was cleared.
    pub batch: Batch,
    /// The clearing, with its certificate.
    pub result: ClearingResult,
    /// The price of a unit: none where nothing traded.
    pub price: Option<Amount>,
    /// Who works for whom.
    pub pairs: Vec<Pair>,
}

impl Cleared {
    /// The digest of the batch.
    pub fn digest(&self) -> String {
        let bytes = borsh::to_vec(&self.batch).unwrap_or_default();
        sion_core::hash::hash_domain("sion-cli/work-market/batch/v1", &bytes).to_hex()
    }

    /// What a claim records of it.
    pub fn recorded(&self, market: &ClaimId, round: u64) -> Recorded {
        Recorded {
            market: market.as_hash().to_hex(),
            round,
            price: self.price.and_then(|p| u64::try_from(p.micros()).ok()),
            volume: self.result.volume,
            batch: self.digest(),
        }
    }
}

/// The batch `bids` make for clearing, with the beacon that breaks their
/// ties.
pub fn batch_of(
    claim: &ClaimId,
    market: &Market,
    round: u64,
    beacon: Hash32,
    bids: &[Bid],
) -> Batch {
    Batch {
        domain: DomainId::derive_parts(&[b"work-market", &claim.as_hash().0, &round.to_le_bytes()]),
        pulse: Pulse(market.closes(round)),
        beacon,
        orders: bids
            .iter()
            .filter_map(|bid| match bid.standing {
                Standing::Open { quantity, price } => Some(Order {
                    id: *bid.claim.as_hash(),
                    owner: bid.bidder,
                    side: bid.side,
                    quantity,
                    limit_price: Amount(u128::from(price)),
                }),
                _ => None,
            })
            .collect(),
    }
}

/// Clears `batch`: the certified allocation, its one price, and who works
/// for whom.
pub fn clear(batch: Batch) -> Result<Cleared, CliError> {
    let result = sion_clearing::clear(&batch)
        .map_err(|e| CliError::Usage(format!("the round could not be cleared: {e}")))?;
    let verified = sion_clearing::verify_clearing(&batch, &result)
        .map_err(|e| CliError::Verification(format!("the clearing's certificate: {e}")))?;
    let transfers = sion_clearing::settlement(&batch, &result)
        .map_err(|e| CliError::Verification(format!("the clearing's pairs: {e}")))?;
    let mut pairs: Vec<Pair> = Vec::new();
    if let Some(price) = verified.price.filter(|p| p.micros() > 0) {
        for transfer in transfers {
            let units = (transfer.amount.micros() / price.micros()) as u64;
            match pairs
                .iter_mut()
                .find(|p| p.buyer == transfer.from && p.worker == transfer.to)
            {
                Some(pair) => pair.units += units,
                None => pairs.push(Pair {
                    buyer: transfer.from,
                    worker: transfer.to,
                    units,
                }),
            }
        }
    }
    Ok(Cleared {
        batch,
        result,
        price: verified.price,
        pairs,
    })
}

/// The clearing of `round` of the market `claim`, from what the ledger
/// holds: `None` until the round has closed.
pub fn cleared(
    ledger: &dyn Ledger,
    claim: ClaimId,
    market: &Market,
    round: u64,
) -> Result<Option<(Vec<Bid>, Cleared)>, CliError> {
    let closes = market.closes(round);
    if ledger.pulse()?.0 < closes {
        return Ok(None);
    }
    let beacon = ledger.pulse_view(Some(Pulse(closes)))?.header.beacon;
    let bids = bids(ledger, claim, market, round)?;
    let cleared = clear(batch_of(&claim, market, round, beacon, &bids))?;
    Ok(Some((bids, cleared)))
}

/// How a pair followed a clearing through: the tasks its buyer posted to
/// its worker at the round's price, and those the worker was paid for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Followed {
    /// The tasks posted, at most the pair's units.
    pub posted: Vec<ObjectId>,
    /// How many of those were settled: results accepted and paid.
    pub settled: u64,
}

/// Reads from the ledger how each of `pairs` followed the clearing of
/// `round` through: tasks posted by the buyer to the worker for exactly
/// `price`, after the round closed and within the time the market gives,
/// counted once each, and which of them were settled.
pub fn followed(
    ledger: &dyn Ledger,
    market: &Market,
    round: u64,
    price: Amount,
    pairs: &[Pair],
) -> Result<Vec<Followed>, CliError> {
    let closes = market.closes(round);
    let mut all = vec![Followed::default(); pairs.len()];
    let mut from = closes;
    loop {
        let page = ledger.events(Pulse(from), 5_000)?;
        let Some(last) = page.last().map(|view| view.pulse.0) else {
            break;
        };
        for view in &page {
            match &view.event {
                Event::TaskPosted {
                    task,
                    poster,
                    worker: Some(worker),
                    amount,
                    ..
                } if *amount == price && view.pulse.0 <= closes + market.post_within => {
                    let free = pairs.iter().zip(all.iter_mut()).find(|(pair, had)| {
                        pair.buyer == *poster
                            && pair.worker == *worker
                            && (had.posted.len() as u64) < pair.units
                            && !had.posted.contains(task)
                    });
                    if let Some((_, had)) = free {
                        had.posted.push(*task);
                    }
                }
                Event::TaskSettled { task, .. } => {
                    if let Some(had) = all.iter_mut().find(|had| had.posted.contains(task)) {
                        had.settled += 1;
                    }
                }
                _ => {}
            }
        }
        if page.len() < 5_000 {
            break;
        }
        // A full page may end inside a Pulse: read on from the next one.
        from = last + 1;
    }
    Ok(all)
}

/// A claim that rests on its issuer's signature alone.
fn signed(schema: &str, subject: String, payload: Vec<u8>) -> ClaimBody {
    ClaimBody {
        schema: schema.into(),
        subject,
        payload: Payload::Public(payload),
        formula: sion_assurance::Formula::and(vec![formulas::dual_signature()]),
        adjudicator: None,
        detection_permille: 0,
        valid_for: 1_000_000,
        challenge_period: 0,
    }
}

fn written<T: Serialize>(value: &T) -> Result<Vec<u8>, CliError> {
    serde_json::to_vec(value).map_err(|e| CliError::Usage(format!("it could not be written: {e}")))
}

/// Opens `market` as `wallet`'s claim.
pub fn open(session: &mut Session<'_>, wallet: &str, market: &Market) -> Result<ClaimId, CliError> {
    market.check().map_err(CliError::Usage)?;
    let body = signed(
        MARKET_SCHEMA,
        format!("a market for work: {}", market.name),
        written(market)?,
    );
    crate::vouching::publish(session, wallet, body, Some(market_key(&market.name)))
}

/// The market the claim `id` opens.
pub fn market(ledger: &dyn Ledger, id: &str) -> Result<(ClaimId, Market, AccountId), CliError> {
    let claim = Hash32::from_hex(id.trim())
        .map(ClaimId::from_hash)
        .map_err(|e| CliError::Usage(format!("{id:?} is not a market's claim: {e}")))?;
    let read = feeds::read::<Market>(ledger, claim, MARKET_SCHEMA)?.ok_or_else(|| {
        CliError::Usage(format!("claim {} is not a market for work", claim.short()))
    })?;
    read.says.check().map_err(CliError::Usage)?;
    Ok((claim, read.says, read.issuer))
}

/// Publishes `sealed` as `wallet`'s bid.
pub fn bid(
    session: &mut Session<'_>,
    wallet: &str,
    claim: &ClaimId,
    sealed: &SealedBid,
) -> Result<ClaimId, CliError> {
    let body = signed(
        BID_SCHEMA,
        format!(
            "a sealed bid to {} work in round {} of market {}",
            side_word(sealed.side),
            sealed.round,
            claim.short()
        ),
        written(sealed)?,
    );
    crate::vouching::publish(session, wallet, body, Some(bid_key(claim, sealed.round)))
}

/// Publishes the reveal of `secret`, a bid of `wallet`'s.
pub fn reveal(
    session: &mut Session<'_>,
    wallet: &str,
    claim: &ClaimId,
    secret: &Secret,
) -> Result<ClaimId, CliError> {
    let Some(bid) = &secret.claim else {
        return Err(CliError::Usage("that bid was never published".into()));
    };
    let body = signed(
        REVEAL_SCHEMA,
        format!(
            "reveals a bid in round {} of market {}",
            secret.round,
            claim.short()
        ),
        written(&Revealed {
            bid: bid.clone(),
            salt: secret.salt.clone(),
            price: secret.price.clone(),
            quantity: secret.quantity.clone(),
        })?,
    );
    crate::vouching::publish(session, wallet, body, Some(reveal_key(claim, secret.round)))
}

/// Records what `round` cleared at, as `wallet`'s claim.
pub fn record(
    session: &mut Session<'_>,
    wallet: &str,
    claim: &ClaimId,
    round: u64,
    cleared: &Cleared,
) -> Result<ClaimId, CliError> {
    let recorded = cleared.recorded(claim, round);
    let body = signed(
        CLEARED_SCHEMA,
        match cleared.price {
            Some(price) => format!(
                "round {round} of market {} cleared: {} unit(s) at {price} USD",
                claim.short(),
                recorded.volume
            ),
            None => format!(
                "round {round} of market {} cleared: nothing traded",
                claim.short()
            ),
        },
        written(&recorded)?,
    );
    crate::vouching::publish(session, wallet, body, Some(cleared_key(claim, round)))
}

/// The task a buyer posts for unit `unit` of its pair with `worker`: what
/// its spec commits to, the buyer's own description where it gives one.
fn spec_of(
    claim: &ClaimId,
    market: &Market,
    round: u64,
    worker: &AccountId,
    unit: u64,
    item: &[u8],
) -> Hash32 {
    let said = serde_json::json!({
        "market": claim.as_hash().to_hex(),
        "unit_of_work": market.unit,
        "round": round,
        "worker": worker.as_hash().to_hex(),
        "unit": unit,
        "item": hex::encode(sion_core::hash::hash_domain("sion-cli/work-market/item/v1", item).0),
    });
    crate::tasks::spec_commitment(said.to_string().as_bytes())
}

/// Posts the tasks `wallet` owes from `cleared`, the clearing of `round`:
/// one for each unit it bought, to the worker it was paired with, at the
/// round's price, under the market's acceptance. Tasks already posted are
/// not posted again. Returns the tasks posted now.
pub fn post(
    session: &mut Session<'_>,
    wallet: &str,
    claim: &ClaimId,
    market: &Market,
    round: u64,
    cleared: &Cleared,
    item: &[u8],
) -> Result<Vec<(AccountId, ObjectId)>, CliError> {
    let Some(price) = cleared.price else {
        return Ok(Vec::new());
    };
    let ledger = session.ledger;
    let me = session.id(wallet);
    let had = followed(ledger, market, round, price, &cleared.pairs)?;
    let mut posted = Vec::new();
    for (pair, had) in cleared.pairs.iter().zip(had) {
        if pair.buyer != me {
            continue;
        }
        for unit in had.posted.len() as u64..pair.units {
            let view = session.must(
                wallet,
                TxKind::PostTask {
                    spec: spec_of(claim, market, round, &pair.worker, unit, item),
                    acceptance: market.acceptance.clone(),
                    worker: Some(pair.worker),
                    amount: price,
                    deadline: Pulse(ledger.pulse()?.0 + market.deadline),
                    charter: None,
                },
                Vec::new(),
            )?;
            let task = view
                .effects
                .events
                .iter()
                .find_map(|event| match event {
                    Event::TaskPosted { task, .. } => Some(*task),
                    _ => None,
                })
                .ok_or_else(|| CliError::Transaction("the ledger posted no task".into()))?;
            posted.push((pair.worker, task));
        }
    }
    Ok(posted)
}

fn secrets_dir(claim: &ClaimId) -> PathBuf {
    crate::setup::home().join("markets").join(short(claim))
}

fn secret_path(claim: &ClaimId, round: u64, wallet: &str, side: Side) -> PathBuf {
    secrets_dir(claim).join(format!("{round}-{wallet}-{}.json", side_word(side)))
}

/// Where a market stands, in lines: its terms, and for each of `rounds`
/// its bids, its price, who works for whom and how each pair followed
/// through.
pub fn lines(
    ledger: &dyn Ledger,
    claim: ClaimId,
    market: &Market,
    convener: &AccountId,
    rounds: impl IntoIterator<Item = u64>,
) -> Result<Vec<String>, CliError> {
    let now = ledger.pulse()?.0;
    let usd = |micros: u64| Amount(u128::from(micros)).to_string();
    let mut lines = vec![
        format!(
            "market {} by {}: {}",
            market.name,
            convener.short(),
            market.unit.split_whitespace().collect::<Vec<_>>().join(" ")
        ),
        format!(
            "  prices from {} to {} USD a unit, at most {} unit(s) a bid; rounds of {} Pulse(s) \
             to bid and {} to reveal, from Pulse {}",
            usd(market.min_price),
            usd(market.max_price),
            market.max_quantity,
            market.bidding,
            market.revealing,
            market.first
        ),
    ];
    for round in rounds {
        let bids = bids(ledger, claim, market, round)?;
        let counted = |side: Side| {
            bids.iter()
                .filter(|b| b.side == side && !matches!(b.standing, Standing::Out(_)))
                .count()
        };
        let phase = if now < market.bids_from(round) {
            format!("opens at Pulse {}", market.bids_from(round))
        } else if now < market.reveals_from(round) {
            format!("taking bids until Pulse {}", market.reveals_from(round))
        } else if now < market.closes(round) {
            format!("taking reveals until Pulse {}", market.closes(round))
        } else {
            format!("closed at Pulse {}", market.closes(round))
        };
        lines.push(format!(
            "round {round}: {phase}; {} sealed bid(s) to buy and {} to sell",
            counted(Side::Buy),
            counted(Side::Sell)
        ));
        for bid in &bids {
            if let Standing::Out(why) = &bid.standing {
                lines.push(format!(
                    "  left out: bid {} by {}: {why}",
                    bid.claim.short(),
                    bid.bidder.short()
                ));
            }
        }
        let Some((bids, cleared)) = cleared(ledger, claim, market, round)? else {
            continue;
        };
        for bid in &bids {
            match bid.standing {
                Standing::Open { quantity, price } => lines.push(format!(
                    "  {} by {}: {quantity} unit(s) at {} USD",
                    side_word(bid.side),
                    bid.bidder.short(),
                    usd(price)
                )),
                Standing::Sealed => lines.push(format!(
                    "  never revealed: bid {} by {}",
                    bid.claim.short(),
                    bid.bidder.short()
                )),
                Standing::Out(_) => {}
            }
        }
        let Some(price) = cleared.price else {
            lines.push("  nothing traded".to_string());
            continue;
        };
        lines.push(format!(
            "  cleared: {} unit(s) at {price} USD a unit",
            cleared.result.volume
        ));
        let followed = followed(ledger, market, round, price, &cleared.pairs)?;
        for (pair, had) in cleared.pairs.iter().zip(followed) {
            let late = now > market.closes(round) + market.post_within;
            let owed = pair.units - had.posted.len() as u64;
            lines.push(format!(
                "  {} buys {} unit(s) from {}: {} task(s) posted, {} paid for{}",
                pair.buyer.short(),
                pair.units,
                pair.worker.short(),
                had.posted.len(),
                had.settled,
                match (owed, late) {
                    (0, _) => String::new(),
                    (owed, true) => format!("; {owed} never posted"),
                    (owed, false) => format!(
                        "; {owed} to post by Pulse {}",
                        market.closes(round) + market.post_within
                    ),
                }
            ));
        }
        let mut feed = Feed {
            next: market.closes(round),
            ..Feed::default()
        };
        let key = cleared_key(&claim, round);
        feed.scan(ledger, &[&key])?;
        let ours = cleared.recorded(&claim, round);
        for entry in feed.entries.iter().filter(|e| e.key == key) {
            let Some(read) = feeds::read::<Recorded>(ledger, entry.claim, CLEARED_SCHEMA)? else {
                continue;
            };
            if read.issuer != entry.owner
                || read.says.market != ours.market
                || read.says.round != round
            {
                continue;
            }
            lines.push(if read.says == ours {
                format!(
                    "  recorded by {} in claim {}",
                    read.issuer.short(),
                    entry.claim.short()
                )
            } else {
                format!(
                    "  claim {} by {} records another clearing than the ledger's bids give",
                    entry.claim.short(),
                    read.issuer.short()
                )
            });
        }
    }
    Ok(lines)
}

/// Which side a bid is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum BidSide {
    /// Buy work: pay at most the price for each unit.
    Buy,
    /// Offer work: take at least the price for each unit.
    Sell,
}

impl From<BidSide> for Side {
    fn from(side: BidSide) -> Side {
        match side {
            BidSide::Buy => Side::Buy,
            BidSide::Sell => Side::Sell,
        }
    }
}

/// `sion agent market`.
#[derive(Clone, Debug, Subcommand)]
pub enum MarketCommand {
    /// Open a market for one kind of work: what a unit is, the rule a
    /// result must pass to be paid, a price band, and how long each round
    /// takes bids and reveals.
    Open {
        /// The market's name (lowercase letters, digits and hyphens).
        name: String,
        /// What one unit of work is, in words a worker can price.
        #[arg(long)]
        unit: String,
        /// The lowest price of a unit a bid may name, in USD.
        #[arg(long = "min-price", value_name = "USD")]
        min_price: String,
        /// The highest.
        #[arg(long = "max-price", value_name = "USD")]
        max_price: String,
        /// The most units one bid may be for.
        #[arg(long = "max-quantity", default_value_t = 100)]
        max_quantity: u64,
        /// How many Pulses each round takes bids for.
        #[arg(long, default_value_t = 20)]
        bidding: u64,
        /// How many Pulses, after those, it takes reveals for.
        #[arg(long, default_value_t = 20)]
        revealing: u64,
        /// The first Pulse of the first round (the next Pulse, if not
        /// given).
        #[arg(long)]
        first: Option<u64>,
        /// Verifiers whose attestations of a result count (wallets or
        /// accounts; repeat for more).
        #[arg(long = "verifier", value_name = "ACCOUNT")]
        verifiers: Vec<String>,
        /// How many attestations a result needs at least.
        #[arg(long = "min-attestations", default_value_t = 0)]
        min_attestations: u8,
        /// The adjudicator atom results must name (hex).
        #[arg(long)]
        adjudicator: Option<String>,
        /// Pulses of reliance on a paid result.
        #[arg(long, default_value_t = 1_000)]
        reliance: u64,
        /// How many Pulses after a round closes its buyers have to post
        /// their tasks.
        #[arg(long = "post-within", default_value_t = 100)]
        post_within: u64,
        /// How many Pulses a posted task's worker has to settle it.
        #[arg(long, default_value_t = 2_000)]
        deadline: u64,
        /// Your wallet.
        #[arg(long)]
        wallet: String,
    },
    /// Make a sealed bid in the round now taking bids: to buy units of
    /// work at no more than a price, or to offer them at no less. The
    /// price and quantity stay on this machine until you reveal them; what
    /// is published commits to them and proves they are inside the
    /// market's band.
    Bid {
        /// The market (its claim).
        market: String,
        /// Which side.
        #[arg(long, value_enum)]
        side: BidSide,
        /// How many units.
        #[arg(long)]
        quantity: u64,
        /// The price of a unit, in USD: the most you pay, or the least you
        /// take.
        #[arg(long, value_name = "USD")]
        price: String,
        /// Your wallet.
        #[arg(long)]
        wallet: String,
    },
    /// Reveal your bids of the round now taking reveals. A bid that is not
    /// revealed in time is left out of its round.
    Reveal {
        /// The market (its claim).
        market: String,
        /// Your wallet.
        #[arg(long)]
        wallet: String,
    },
    /// Clear a closed round from the bids the ledger holds: the certified
    /// allocation, its one price, and who works for whom. Anyone computes
    /// the same; with --wallet, record it in a claim.
    Clear {
        /// The market (its claim).
        market: String,
        /// The round.
        round: u64,
        /// Record the clearing as this wallet's claim.
        #[arg(long)]
        wallet: Option<String>,
    },
    /// Post the tasks you owe from a cleared round: one for each unit you
    /// bought, to the worker you were paired with, at the round's price,
    /// under the market's acceptance. The price is escrowed now and paid
    /// for an accepted result.
    Post {
        /// The market (its claim).
        market: String,
        /// The round.
        round: u64,
        /// A file describing your work items, committed to in each task's
        /// spec.
        #[arg(long, value_name = "FILE")]
        item: Option<PathBuf>,
        /// Your wallet.
        #[arg(long)]
        wallet: String,
    },
    /// Where a market stands: its terms, each round's bids and price, who
    /// works for whom, and who posted the tasks they owed and was paid.
    Show {
        /// The market (its claim).
        market: String,
        /// A round to show (every round so far, if not given).
        #[arg(long)]
        round: Option<u64>,
    },
}

/// Runs a market command.
pub fn command(
    session: &mut Session<'_>,
    ledger: &dyn Ledger,
    command: MarketCommand,
) -> Result<(), CliError> {
    match command {
        MarketCommand::Open {
            name,
            unit,
            min_price,
            max_price,
            max_quantity,
            bidding,
            revealing,
            first,
            verifiers,
            min_attestations,
            adjudicator,
            reliance,
            post_within,
            deadline,
            wallet,
        } => {
            let micros = |text: &str| -> Result<u64, CliError> {
                u64::try_from(crate::agents::parse_amount(text)?.micros())
                    .map_err(|_| CliError::Usage(format!("{text} USD is more than a price can be")))
            };
            let market = Market {
                name,
                unit,
                acceptance: Acceptance {
                    policy: sion_assurance::Policy::standard(),
                    requires: Default::default(),
                    verifiers: verifiers.iter().map(|v| session.id(v)).collect(),
                    must_attest: Default::default(),
                    min_attestations,
                    adjudicator: adjudicator
                        .as_deref()
                        .map(|hex| {
                            Hash32::from_hex(hex).map(AtomId::from_hash).map_err(|e| {
                                CliError::Usage(format!("the adjudicator's atom: {e}"))
                            })
                        })
                        .transpose()?,
                    detection_permille: 900,
                    reliance,
                },
                first: first.unwrap_or(ledger.pulse()?.0 + 1),
                bidding,
                revealing,
                min_price: micros(&min_price)?,
                max_price: micros(&max_price)?,
                max_quantity,
                post_within,
                deadline,
            };
            let claim = open(session, &wallet, &market)?;
            println!(
                "opened market {}: claim {}",
                market.name,
                claim.as_hash().to_hex()
            );
            println!(
                "  round 0 takes bids from Pulse {} and reveals from Pulse {}, and closes at Pulse {}",
                market.bids_from(0),
                market.reveals_from(0),
                market.closes(0)
            );
            Ok(())
        }
        MarketCommand::Bid {
            market: id,
            side,
            quantity,
            price,
            wallet,
        } => {
            let (claim, market, _) = market(ledger, &id)?;
            let now = ledger.pulse()?.0;
            // A bid lands a Pulse or more after it is signed.
            let round = match market.round_at(now + 1) {
                Some(round) if now + 1 < market.reveals_from(round) => round,
                Some(round) => {
                    return Err(CliError::Usage(format!(
                        "round {round} no longer takes bids: round {} does from Pulse {}",
                        round + 1,
                        market.bids_from(round + 1)
                    )))
                }
                None => {
                    return Err(CliError::Usage(format!(
                        "the market opens at Pulse {}",
                        market.first
                    )))
                }
            };
            let side = Side::from(side);
            let path = secret_path(&claim, round, &wallet, side);
            if path.exists() {
                return Err(CliError::Usage(format!(
                    "{wallet} already has a bid to {} in round {round}",
                    side_word(side)
                )));
            }
            let bidder = session.id(&wallet);
            let (sealed, mut secret) = seal(
                &claim,
                &market,
                round,
                &bidder,
                side,
                quantity,
                crate::agents::parse_amount(&price)?,
            )?;
            // Kept before it is published: a bid whose secret is lost
            // cannot be revealed.
            crate::profiles::write_json(&path, &secret)?;
            let published = bid(session, &wallet, &claim, &sealed)?;
            secret.claim = Some(published.as_hash().to_hex());
            crate::profiles::write_json(&path, &secret)?;
            println!(
                "sealed bid to {} in round {round}: claim {}",
                side_word(side),
                published.short()
            );
            println!(
                "  its quantity and price are kept in {}; reveal them from Pulse {} and before \
                 Pulse {} (sion agent market reveal)",
                path.display(),
                market.reveals_from(round),
                market.closes(round)
            );
            Ok(())
        }
        MarketCommand::Reveal { market: id, wallet } => {
            let (claim, market, _) = market(ledger, &id)?;
            let now = ledger.pulse()?.0;
            let round = market
                .round_at(now + 1)
                .filter(|round| now + 1 >= market.reveals_from(*round))
                .ok_or_else(|| {
                    CliError::Usage("no round of this market takes reveals now".into())
                })?;
            let mut revealed = 0;
            for side in [Side::Buy, Side::Sell] {
                let path = secret_path(&claim, round, &wallet, side);
                if !path.exists() {
                    continue;
                }
                let text = std::fs::read_to_string(&path)
                    .map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?;
                let secret: Secret = serde_json::from_str(&text)
                    .map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?;
                let said = reveal(session, &wallet, &claim, &secret)?;
                println!(
                    "revealed your bid to {} in round {round}: {} unit(s) at {} USD (claim {})",
                    side_word(side),
                    secret.quantity.value,
                    Amount(u128::from(secret.price.value)),
                    said.short()
                );
                revealed += 1;
            }
            if revealed == 0 {
                return Err(CliError::Usage(format!(
                    "{wallet} has no bid in round {round} on this machine"
                )));
            }
            println!("  the round closes at Pulse {}", market.closes(round));
            Ok(())
        }
        MarketCommand::Clear {
            market: id,
            round,
            wallet,
        } => {
            let (claim, market, _) = market(ledger, &id)?;
            let Some((bids, cleared)) = cleared(ledger, claim, &market, round)? else {
                return Err(CliError::Usage(format!(
                    "round {round} closes at Pulse {}",
                    market.closes(round)
                )));
            };
            let open = bids
                .iter()
                .filter(|b| matches!(b.standing, Standing::Open { .. }))
                .count();
            match cleared.price {
                Some(price) => println!(
                    "round {round} cleared: {} unit(s) at {price} USD a unit, from {open} \
                     revealed bid(s); the certificate checks",
                    cleared.result.volume
                ),
                None => println!("round {round}: nothing traded, from {open} revealed bid(s)"),
            }
            for pair in &cleared.pairs {
                println!(
                    "  {} buys {} unit(s) from {}",
                    pair.buyer.short(),
                    pair.units,
                    pair.worker.short()
                );
            }
            if let Some(wallet) = wallet {
                let said = record(session, &wallet, &claim, round, &cleared)?;
                println!("recorded in claim {}", said.as_hash().to_hex());
            }
            Ok(())
        }
        MarketCommand::Post {
            market: id,
            round,
            item,
            wallet,
        } => {
            let (claim, market, _) = market(ledger, &id)?;
            let Some((_, cleared)) = cleared(ledger, claim, &market, round)? else {
                return Err(CliError::Usage(format!(
                    "round {round} closes at Pulse {}",
                    market.closes(round)
                )));
            };
            let item = match item {
                Some(path) => std::fs::read(&path)
                    .map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?,
                None => Vec::new(),
            };
            let posted = post(session, &wallet, &claim, &market, round, &cleared, &item)?;
            if posted.is_empty() {
                println!("{wallet} owes no task from round {round}");
            }
            for (worker, task) in &posted {
                println!(
                    "posted task {task} to {}: {} USD escrowed, paid for an accepted result",
                    worker.short(),
                    cleared.price.unwrap_or_default()
                );
            }
            Ok(())
        }
        MarketCommand::Show { market: id, round } => {
            let (claim, market, convener) = market(ledger, &id)?;
            let now = ledger.pulse()?.0;
            let rounds: Vec<u64> = match round {
                Some(round) => vec![round],
                None => (0..=market.round_at(now).unwrap_or(0)).collect(),
            };
            for line in lines(ledger, claim, &market, &convener, rounds)? {
                println!("{line}");
            }
            Ok(())
        }
    }
}

/// The prices rounds `rounds` of a market cleared at, where anything
/// traded: the market's public price.
pub fn prices(
    ledger: &dyn Ledger,
    claim: ClaimId,
    market: &Market,
    rounds: impl IntoIterator<Item = u64>,
) -> Result<BTreeMap<u64, Amount>, CliError> {
    let mut prices = BTreeMap::new();
    for round in rounds {
        if let Some((_, cleared)) = cleared(ledger, claim, market, round)? {
            if let Some(price) = cleared.price {
                prices.insert(round, price);
            }
        }
    }
    Ok(prices)
}
