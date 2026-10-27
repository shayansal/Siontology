//! The deferred phase (spec §6.3): the last chunk of a Pulse. It sorts and
//! applies every transaction's deferred items (atoms, bonds, claims,
//! orders, returns, treasury credits, supply records, schedule entries),
//! posts the Pulse's forced intents as obligations of their senders'
//! shards, then processes the schedule's due events (batch sealing,
//! clearing deadlines, registry ticks, forced-inclusion deadlines with the
//! committee rotations they cause, epoch ends).
//!
//! Protocol objects are read lazily: the schedule always and first; the
//! assurance domain, the treasury, the validator set and the
//! forced-inclusion obligations only when an item, an intent or an event
//! needs them; a market only when an item or event needs it. Each is written
//! back at the end only if something obtained it (the schedule: only if its
//! entries changed), in the order markets (by identifier), assurance,
//! treasury, validator set, obligations, schedule.
//!
//! Errors (§6.3, "Errors"): the arithmetic §6.3 marks saturating saturates;
//! any remaining error — a missing or malformed protocol object, an
//! overflowing `unallocated`, a failure of the registry's own accounting, a
//! committee that cannot be sampled — makes the deferred phase invalid, so
//! the chunk is rejected.

mod epoch;
mod forced;

use std::collections::BTreeMap;

use sion_assurance::{ClaimRecord, ClaimStatus, Ratio};
use sion_core::encode::encode;
use sion_core::hash::Hash32;
use sion_core::ids::{AccountId, AssetId, ObjectId, Pulse, Version};
use sion_core::merkle::Frontier;
use sion_core::Amount;
use sion_exec::assurance::AssuranceDomain;
use sion_exec::claim::{claim_id, knowledge_leaf, ClaimBody, StoredClaim};
use sion_exec::execute::{bond_owner, Deferred, Event};
use sion_exec::market::{BatchStatus, Market, MarketBatch, PendingOrder, MAX_BATCH_ORDERS};
use sion_exec::object::{
    well_known, Coin, Object, ObjectData, Owner, Schedule, ScheduledEvent, Treasury,
};
use sion_exec::tx::SignedTransaction;
use sion_exec::validators::{ForcedObligations, ValidatorSet};

use crate::commit::deferred_coin_id;
use crate::context::SpecContext;
use crate::error::{Abort, SpecError};
use crate::journal::Scratch;
use crate::view::ChunkView;

/// What the deferred phase produced.
#[derive(Debug)]
pub(crate) struct DeferredOutcome {
    /// Its writes.
    pub writes: BTreeMap<ObjectId, Object>,
    /// Identifiers it read from the witness.
    pub reads: std::collections::BTreeSet<ObjectId>,
    /// The knowledge frontier after appending the Pulse's claims.
    pub frontier: Frontier,
    /// Events (not committed; for reports).
    pub events: Vec<Event>,
}

/// A failure of the deferred phase: it has no per-item outcome, so the
/// chunk is rejected.
fn broken(message: impl Into<String>) -> SpecError {
    SpecError::DeferredPhase(message.into())
}

/// Turns an execution stop inside the deferred phase into a chunk
/// rejection (a witness gap stays what it is).
fn rejected(abort: Abort) -> SpecError {
    match abort {
        Abort::Chunk(error) => error,
        Abort::Op(error) => broken(error.0),
    }
}

/// Checked amount addition in the deferred phase.
fn add(a: Amount, b: Amount, what: &str) -> Result<Amount, SpecError> {
    a.checked_add(b)
        .map_err(|_| broken(format!("{what} overflows")))
}

/// Checked amount subtraction in the deferred phase.
fn sub(a: Amount, b: Amount, what: &str) -> Result<Amount, SpecError> {
    a.checked_sub(b)
        .map_err(|_| broken(format!("{what} underflows")))
}

/// The sort key of a deferred item: `(rank, natural key, enc(item))`.
pub(crate) fn sort_key(item: &Deferred) -> (u8, Vec<u8>, Vec<u8>) {
    let (rank, natural) = match item {
        Deferred::RegisterAtom { atom } => (0, encode(&atom.id)),
        Deferred::PostBond { atom, owner, .. } => (1, encode(&(atom.id, *owner))),
        Deferred::IssueClaim { issuer, body, .. } => (2, encode(&claim_id(issuer, body))),
        Deferred::AddOrder { order, .. } => (3, encode(&order.id)),
        Deferred::ReturnToBounded { bounded, .. } => (4, encode(bounded)),
        Deferred::Treasury { asset, .. } => (5, encode(asset)),
        Deferred::Minted { asset, .. } => (6, encode(asset)),
        Deferred::Schedule { at, event } => (7, encode(&(*at, *event))),
    };
    (rank, natural, encode(item))
}

/// The deferred phase in progress.
struct DeferredRun<'v> {
    /// The Pulse context.
    ctx: &'v SpecContext,
    /// Bookkeeping as in §3.1 (fresh originals and created).
    book: Scratch<'v>,
    /// `k`, the payout counter.
    payouts: u64,
    /// The schedule object as loaded.
    schedule_object: Object,
    /// Its entries.
    schedule: Schedule,
    /// Whether the schedule's entries changed.
    schedule_changed: bool,
    /// The assurance object and its domain, once obtained.
    assurance: Option<(Object, AssuranceDomain)>,
    /// The treasury object and its contents, once obtained.
    treasury: Option<(Object, Treasury)>,
    /// The validator set object and its contents, once obtained.
    validators: Option<(Object, ValidatorSet)>,
    /// The forced-inclusion obligations object and its contents, once
    /// obtained.
    obligations: Option<(Object, ForcedObligations)>,
    /// Markets obtained so far, by identifier.
    markets: BTreeMap<ObjectId, (Object, Market)>,
    /// The knowledge log's frontier.
    frontier: Frontier,
    /// Events.
    events: Vec<Event>,
}

impl<'v> DeferredRun<'v> {
    /// Starts the phase: the schedule object is loaded first.
    fn start(
        ctx: &'v SpecContext,
        view: &'v ChunkView,
        frontier: Frontier,
    ) -> Result<Self, SpecError> {
        let mut book = Scratch::new(view);
        let schedule_object = book.load(&well_known::schedule()).map_err(rejected)?;
        let ObjectData::Schedule(schedule) = &schedule_object.data else {
            return Err(broken("the schedule object is not a schedule"));
        };
        let schedule = schedule.clone();
        Ok(DeferredRun {
            ctx,
            book,
            payouts: 0,
            schedule_object,
            schedule,
            schedule_changed: false,
            assurance: None,
            treasury: None,
            validators: None,
            obligations: None,
            markets: BTreeMap::new(),
            frontier,
            events: Vec::new(),
        })
    }

    /// The assurance domain, loading it the first time.
    fn assurance(&mut self) -> Result<&mut AssuranceDomain, SpecError> {
        if self.assurance.is_none() {
            let object = self.book.load(&well_known::registry()).map_err(rejected)?;
            let ObjectData::Assurance(domain) = &object.data else {
                return Err(broken("the registry object is not the assurance domain"));
            };
            let domain = (**domain).clone();
            self.assurance = Some((object, domain));
        }
        match &mut self.assurance {
            Some((_, domain)) => Ok(domain),
            None => Err(broken("assurance domain unavailable")),
        }
    }

    /// The treasury, loading it the first time.
    fn treasury(&mut self) -> Result<&mut Treasury, SpecError> {
        if self.treasury.is_none() {
            let object = self.book.load(&well_known::treasury()).map_err(rejected)?;
            let ObjectData::Treasury(treasury) = &object.data else {
                return Err(broken("the treasury object is not a treasury"));
            };
            let treasury = treasury.clone();
            self.treasury = Some((object, treasury));
        }
        match &mut self.treasury {
            Some((_, treasury)) => Ok(treasury),
            None => Err(broken("treasury unavailable")),
        }
    }

    /// The validator set, loading it the first time.
    fn validator_set(&mut self) -> Result<&mut ValidatorSet, SpecError> {
        if self.validators.is_none() {
            let object = self
                .book
                .load(&well_known::validators())
                .map_err(rejected)?;
            let ObjectData::Validators(set) = &object.data else {
                return Err(broken("the validator set object is not a validator set"));
            };
            let set = (**set).clone();
            self.validators = Some((object, set));
        }
        match &mut self.validators {
            Some((_, set)) => Ok(set),
            None => Err(broken("validator set unavailable")),
        }
    }

    /// The forced-inclusion obligations, loading them the first time.
    fn obligations(&mut self) -> Result<&mut ForcedObligations, SpecError> {
        if self.obligations.is_none() {
            let object = self.book.load(&well_known::forced()).map_err(rejected)?;
            let ObjectData::Forced(obligations) = &object.data else {
                return Err(broken(
                    "the forced-inclusion object does not hold obligations",
                ));
            };
            let obligations = (**obligations).clone();
            self.obligations = Some((object, obligations));
        }
        match &mut self.obligations {
            Some((_, obligations)) => Ok(obligations),
            None => Err(broken("obligations unavailable")),
        }
    }

    /// The market `id`, loading it the first time; `None` if the object is
    /// missing or not a market (then nothing is obtained).
    fn market(&mut self, id: &ObjectId) -> Result<Option<&mut Market>, SpecError> {
        if !self.markets.contains_key(id) {
            let Some(object) = self.book.try_load(id).map_err(rejected)? else {
                return Ok(None);
            };
            let ObjectData::Market(market) = &object.data else {
                return Ok(None);
            };
            let market = (**market).clone();
            self.markets.insert(*id, (object, market));
        }
        Ok(self.markets.get_mut(id).map(|(_, market)| market))
    }

    /// **pay(owner, asset, amount)**: a new coin under the next deferred
    /// coin identifier, unless `amount = 0`.
    fn pay(&mut self, owner: AccountId, asset: AssetId, amount: Amount) -> Result<(), SpecError> {
        if amount == Amount::ZERO {
            return Ok(());
        }
        let id = deferred_coin_id(self.ctx.pulse, self.payouts);
        self.payouts = self
            .payouts
            .checked_add(1)
            .ok_or_else(|| broken("payout counter overflows"))?;
        self.book
            .store(Object {
                id,
                version: Version(0),
                owner: Owner::Account(owner),
                data: ObjectData::Coin(Coin { asset, amount }),
            })
            .map_err(rejected)
    }

    /// Inserts `event` into `schedule.due[at]`; the schedule changes only
    /// if the event was not already there.
    fn schedule(&mut self, at: Pulse, event: ScheduledEvent) {
        if self.schedule.due.entry(at.0).or_default().insert(event) {
            self.schedule_changed = true;
        }
    }

    /// Applies one deferred item.
    fn item(&mut self, item: &Deferred) -> Result<(), SpecError> {
        match item {
            Deferred::RegisterAtom { atom } => {
                let domain = self.assurance()?;
                if domain.registry.atom(&atom.id).is_none()
                    && domain.registry.register_atom(atom.clone()).is_ok()
                {
                    self.events.push(Event::AtomRegistered { atom: atom.id });
                }
            }
            Deferred::PostBond {
                atom,
                owner,
                amount,
            } => {
                let pulse = self.ctx.pulse;
                let domain = self.assurance()?;
                // A bond the vault cannot hold is refunded before anything is
                // registered or posted.
                let posted = match domain.vault.checked_add(*amount) {
                    Err(_) => false,
                    Ok(vault) => {
                        let acceptable = match domain.registry.atom(&atom.id) {
                            Some(existing) => existing == atom,
                            None => domain.registry.register_atom(atom.clone()).is_ok(),
                        };
                        let posted = acceptable
                            && domain
                                .registry
                                .post_bond(atom.id, *owner, *amount, pulse)
                                .is_ok();
                        if posted {
                            domain.vault = vault;
                        }
                        posted
                    }
                };
                if posted {
                    self.events.push(Event::BondPosted {
                        atom: atom.id,
                        owner: *owner,
                        amount: *amount,
                    });
                } else {
                    self.pay(*owner, AssetId::usd(), *amount)?;
                }
            }
            Deferred::IssueClaim {
                issuer,
                backer,
                body,
            } => self.issue_claim(issuer, backer, body)?,
            Deferred::AddOrder { market, order } => self.add_order(market, order)?,
            Deferred::ReturnToBounded { bounded, amount } => {
                if let Some(mut object) = self.book.try_load(bounded).map_err(rejected)? {
                    if let ObjectData::Bounded(state) = &mut object.data {
                        state.returned = state.returned.saturating_add(*amount);
                        state.unallocated = add(state.unallocated, *amount, "unallocated amount")?;
                        self.book.store(object).map_err(rejected)?;
                    }
                }
            }
            Deferred::Treasury { asset, amount } => {
                let treasury = self.treasury()?;
                let balance = treasury.balances.entry(*asset).or_insert(Amount::ZERO);
                *balance = balance.saturating_add(*amount);
            }
            Deferred::Minted { asset, amount } => {
                let treasury = self.treasury()?;
                let minted = treasury.minted.entry(*asset).or_insert(Amount::ZERO);
                *minted = minted.saturating_add(*amount);
            }
            Deferred::Schedule { at, event } => {
                let at = (*at).max(self.ctx.pulse);
                self.schedule(at, *event);
            }
        }
        Ok(())
    }

    /// The IssueClaim item: checks the claim, registers it and appends it
    /// to the knowledge log, or records its rejection.
    fn issue_claim(
        &mut self,
        issuer: &AccountId,
        backer: &AccountId,
        body: &ClaimBody,
    ) -> Result<(), SpecError> {
        let pulse = self.ctx.pulse;
        let id = claim_id(issuer, body);
        let log_index = self.frontier.size;
        let domain = self.assurance()?;
        let registry = &mut domain.registry;
        let mut reason = None;
        for atom_id in body.formula.direct_atoms() {
            match registry.atom(&atom_id) {
                None => {
                    reason = Some(format!("atom {atom_id} is not registered"));
                    break;
                }
                Some(atom) if atom.bonded && bond_owner(atom) != Some(*backer) => {
                    reason = Some(format!(
                        "bonded atom {atom_id} is not the issuer's (or its backer's)"
                    ));
                    break;
                }
                Some(_) => {}
            }
        }
        if reason.is_none() {
            for referenced in body.formula.claim_refs() {
                match registry.claim(&referenced) {
                    None => reason = Some(format!("referenced claim {referenced} is unknown")),
                    Some(record) if record.issued_at >= pulse => {
                        reason = Some(format!(
                            "referenced claim {referenced} is not from an earlier Pulse"
                        ))
                    }
                    Some(record) if record.status != ClaimStatus::Active => {
                        reason = Some(format!("referenced claim {referenced} is not active"))
                    }
                    Some(_) => {}
                }
                if reason.is_some() {
                    break;
                }
            }
        }
        let valid_until = Pulse(pulse.0.saturating_add(body.valid_for));
        if reason.is_none() {
            let record = ClaimRecord {
                id,
                issuer: *issuer,
                formula: body.formula.clone(),
                adjudicator: body.adjudicator,
                detection: Ratio::permille(u64::from(body.detection_permille)),
                issued_at: pulse,
                valid_until,
                challenge_until: Pulse(valid_until.0.saturating_add(body.challenge_period)),
                status: ClaimStatus::Active,
                sequence: 0,
            };
            if let Err(error) = registry.issue_claim(record) {
                reason = Some(error.to_string());
            }
        }
        if let Some(reason) = reason {
            self.events.push(Event::ClaimRejected { claim: id, reason });
            return Ok(());
        }
        self.frontier.append(knowledge_leaf(&StoredClaim {
            id,
            issuer: *issuer,
            body: body.clone(),
            issued_at: pulse,
            log_index,
        }));
        self.schedule(valid_until, ScheduledEvent::RegistryTick);
        self.events.push(Event::ClaimIssued {
            claim: id,
            log_index,
        });
        Ok(())
    }

    /// The AddOrder item: joins the order to the market's current batch, or
    /// refunds its escrow.
    fn add_order(&mut self, market_id: &ObjectId, order: &PendingOrder) -> Result<(), SpecError> {
        let pulse = self.ctx.pulse;
        let Some(market) = self.market(market_id)? else {
            return self.pay(order.owner, order.escrow_asset, order.escrow);
        };
        let length = market.batch_length.max(1);
        let index = pulse.0 / length;
        // Saturating; the product is at least one, so the subtraction cannot
        // underflow.
        let seal_at = index.saturating_add(1).saturating_mul(length) - 1;
        let batch = market.batches.entry(index).or_insert_with(|| MarketBatch {
            index,
            orders: Vec::new(),
            status: BatchStatus::Open,
        });
        // A batch that is no longer open, or already full, refunds the order.
        if batch.status != BatchStatus::Open || batch.orders.len() >= MAX_BATCH_ORDERS {
            return self.pay(order.owner, order.escrow_asset, order.escrow);
        }
        let was_empty = batch.orders.is_empty();
        let position = batch.orders.partition_point(|other| other.id <= order.id);
        batch.orders.insert(position, order.clone());
        if was_empty {
            self.schedule(
                Pulse(seal_at),
                ScheduledEvent::SealBatch {
                    market: *market_id,
                    batch: index,
                },
            );
        }
        Ok(())
    }

    /// Processes one due scheduled event.
    fn due(&mut self, event: ScheduledEvent) -> Result<(), SpecError> {
        let pulse = self.ctx.pulse;
        match event {
            ScheduledEvent::SealBatch { market, batch } => {
                let beacon: Hash32 = self.ctx.beacon;
                let Some(state) = self.market(&market)? else {
                    return Ok(());
                };
                let deadline = state.clear_deadline;
                let Some(entry) = state.batches.get_mut(&batch) else {
                    return Ok(());
                };
                if entry.status != BatchStatus::Open {
                    return Ok(());
                }
                entry.status = BatchStatus::Sealed { beacon, at: pulse };
                let orders = entry.orders.len() as u64;
                let at = pulse.0.saturating_add(deadline);
                self.events.push(Event::BatchSealed {
                    market,
                    batch,
                    orders,
                });
                self.schedule(Pulse(at), ScheduledEvent::BatchDeadline { market, batch });
            }
            ScheduledEvent::BatchDeadline { market, batch } => {
                let Some(state) = self.market(&market)? else {
                    return Ok(());
                };
                let Some(entry) = state.batches.get_mut(&batch) else {
                    return Ok(());
                };
                if !matches!(entry.status, BatchStatus::Sealed { .. }) {
                    return Ok(());
                }
                entry.status = BatchStatus::Refunded;
                let refunds: Vec<(AccountId, AssetId, Amount)> = entry
                    .orders
                    .iter()
                    .map(|order| (order.owner, order.escrow_asset, order.escrow))
                    .collect();
                for (owner, asset, amount) in refunds {
                    self.pay(owner, asset, amount)?;
                }
                self.events.push(Event::BatchRefunded { market, batch });
            }
            ScheduledEvent::RegistryTick => {
                let next = pulse.0.saturating_add(1);
                let domain = self.assurance()?;
                let expiries = domain.registry.advance_to(Pulse(next));
                let released = expiries
                    .released_pools
                    .values()
                    .try_fold(Amount::ZERO, |total, amount| {
                        add(total, *amount, "released pools")
                    })?;
                if released > Amount::ZERO {
                    domain.vault = sub(domain.vault, released, "vault")?;
                    let treasury = self.treasury()?;
                    let balance = treasury
                        .balances
                        .entry(AssetId::usd())
                        .or_insert(Amount::ZERO);
                    *balance = balance.saturating_add(released);
                }
            }
            ScheduledEvent::ForcedDeadline { intent } => self.forced_deadline(intent)?,
            ScheduledEvent::EpochEnd => self.end_epoch()?,
        }
        Ok(())
    }

    /// Writes back what was used: markets in identifier order, then the
    /// assurance domain, the treasury, the validator set, the obligations,
    /// and the schedule (if its entries changed).
    fn finish(mut self) -> Result<DeferredOutcome, SpecError> {
        let markets = std::mem::take(&mut self.markets);
        for (_, (object, market)) in markets {
            self.book
                .store(Object {
                    data: ObjectData::Market(Box::new(market)),
                    ..object
                })
                .map_err(rejected)?;
        }
        if let Some((object, domain)) = self.assurance.take() {
            self.book
                .store(Object {
                    data: ObjectData::Assurance(Box::new(domain)),
                    ..object
                })
                .map_err(rejected)?;
        }
        if let Some((object, treasury)) = self.treasury.take() {
            self.book
                .store(Object {
                    data: ObjectData::Treasury(treasury),
                    ..object
                })
                .map_err(rejected)?;
        }
        if let Some((object, set)) = self.validators.take() {
            self.book
                .store(Object {
                    data: ObjectData::Validators(Box::new(set)),
                    ..object
                })
                .map_err(rejected)?;
        }
        if let Some((object, obligations)) = self.obligations.take() {
            self.book
                .store(Object {
                    data: ObjectData::Forced(Box::new(obligations)),
                    ..object
                })
                .map_err(rejected)?;
        }
        if self.schedule_changed {
            let object = Object {
                data: ObjectData::Schedule(self.schedule.clone()),
                ..self.schedule_object.clone()
            };
            self.book.store(object).map_err(rejected)?;
        }
        let parts = self.book.finish();
        Ok(DeferredOutcome {
            writes: parts.writes,
            reads: parts.reads,
            frontier: self.frontier,
            events: self.events,
        })
    }
}

/// Runs the deferred phase on `items` (every deferred item of the Pulse, in
/// the order the transactions produced them) and `posted` (the Pulse's
/// posted forced intents, in order), appending claims to `frontier`.
pub(crate) fn run_deferred_phase(
    ctx: &SpecContext,
    view: &ChunkView,
    frontier: Frontier,
    items: &[Deferred],
    posted: &[SignedTransaction],
) -> Result<DeferredOutcome, SpecError> {
    let mut run = DeferredRun::start(ctx, view, frontier)?;
    let mut sorted: Vec<(_, &Deferred)> = items.iter().map(|item| (sort_key(item), item)).collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    for (_, item) in sorted {
        run.item(item)?;
    }
    // After the items, the forced intents, in the order they were posted.
    for intent in posted {
        run.post(intent)?;
    }
    // Every entry with key ≤ ctx.pulse is removed first, then their events
    // run in ascending key order (set order within a key). Events scheduled
    // while processing them land in fresh entries that wait for a later
    // Pulse, even at a key ≤ ctx.pulse.
    let keys: Vec<u64> = run
        .schedule
        .due
        .range(..=ctx.pulse.0)
        .map(|(at, _)| *at)
        .collect();
    let mut due = Vec::with_capacity(keys.len());
    for at in keys {
        if let Some(events) = run.schedule.due.remove(&at) {
            run.schedule_changed = true;
            due.push(events);
        }
    }
    for events in due {
        for event in events {
            run.due(event)?;
        }
    }
    run.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    use sion_assurance::{Atom, AtomClass, Registry};
    use sion_core::ids::DomainId;
    use sion_exec::escrow::BoundedTotal;
    use sion_exec::execute::bond_atom;
    use sion_exec::market::MAX_PERIOD;
    use sion_exec::object::Treasury;
    use sion_exec::state::ChainConfig;

    fn shared(id: ObjectId, data: ObjectData) -> Object {
        Object {
            id,
            version: Version(1),
            owner: Owner::Shared(DomainId::derive(b"test")),
            data,
        }
    }

    fn view(objects: Vec<Object>) -> ChunkView {
        ChunkView::new(objects.into_iter().map(|o| (o.id, Some(o))).collect())
    }

    fn context(pulse: u64) -> SpecContext {
        SpecContext {
            pulse: Pulse(pulse),
            beacon: Hash32::ZERO,
            config: ChainConfig {
                chain_id: "test".into(),
                shard_count: 1,
                base_fee: Amount(1_000),
                fee_per_kib: Amount(100),
                faucet: None,
                max_claim_window: 1_000,
                max_formula_size: 64,
                max_payload_bytes: 1_024,
                challenger_share_permille: 500,
            },
        }
    }

    fn schedule(due: BTreeMap<u64, BTreeSet<ScheduledEvent>>) -> Object {
        shared(
            well_known::schedule(),
            ObjectData::Schedule(Schedule { due }),
        )
    }

    fn assurance(vault: Amount) -> Object {
        shared(
            well_known::registry(),
            ObjectData::Assurance(Box::new(AssuranceDomain {
                registry: Registry::new(),
                vault,
                deposits: BTreeMap::new(),
            })),
        )
    }

    fn domain_of(outcome: &DeferredOutcome) -> AssuranceDomain {
        match &outcome.writes[&well_known::registry()].data {
            ObjectData::Assurance(domain) => (**domain).clone(),
            other => panic!("not the assurance domain: {other:?}"),
        }
    }

    fn market(batch_length: u64, clear_deadline: u64, batches: Vec<MarketBatch>) -> Object {
        shared(
            ObjectId::derive(b"market"),
            ObjectData::Market(Box::new(Market {
                label: "m".into(),
                domain: DomainId::derive(b"market"),
                good: AssetId::derive(b"good"),
                numeraire: AssetId::usd(),
                batch_length,
                clear_deadline,
                committee: Vec::new(),
                threshold: 0,
                batches: batches.into_iter().map(|b| (b.index, b)).collect(),
            })),
        )
    }

    fn market_of(outcome: &DeferredOutcome) -> Market {
        match &outcome.writes[&ObjectId::derive(b"market")].data {
            ObjectData::Market(market) => (**market).clone(),
            other => panic!("not a market: {other:?}"),
        }
    }

    fn pending(tag: &[u8]) -> PendingOrder {
        PendingOrder {
            id: Hash32(sion_core::hash::hash_domain("test/order", tag).0),
            owner: AccountId::derive(tag),
            escrow_asset: AssetId::usd(),
            escrow: Amount(7),
            terms: None,
            sealed: None,
        }
    }

    #[test]
    fn a_bond_the_vault_cannot_hold_is_refunded_before_anything_is_registered() {
        let owner = AccountId::derive(b"owner");
        let atom = bond_atom(AtomClass::Economic, &owner, "stake");
        let items = [Deferred::PostBond {
            atom: atom.clone(),
            owner,
            amount: Amount(10),
        }];
        let ctx = context(9);
        let near_full = Amount(u128::MAX - 5);
        let objects = vec![schedule(BTreeMap::new()), assurance(near_full)];
        let outcome =
            run_deferred_phase(&ctx, &view(objects), Frontier::default(), &items, &[]).unwrap();
        let domain = domain_of(&outcome);
        assert_eq!(domain.vault, near_full);
        assert!(
            domain.registry.atom(&atom.id).is_none(),
            "nothing registered"
        );
        assert!(domain.registry.bond(&atom.id).is_none(), "nothing posted");
        let refund = &outcome.writes[&deferred_coin_id(Pulse(9), 0)];
        assert_eq!(refund.owner, Owner::Account(owner));
        assert_eq!(
            refund.data,
            ObjectData::Coin(Coin {
                asset: AssetId::usd(),
                amount: Amount(10)
            })
        );
        // With room in the vault the same item registers and posts.
        let objects = vec![schedule(BTreeMap::new()), assurance(Amount(1))];
        let outcome =
            run_deferred_phase(&ctx, &view(objects), Frontier::default(), &items, &[]).unwrap();
        let domain = domain_of(&outcome);
        assert_eq!(domain.vault, Amount(11));
        assert!(domain.registry.atom(&atom.id).is_some());
        assert_eq!(domain.registry.bond(&atom.id).unwrap().amount, Amount(10));
        assert!(!outcome.writes.contains_key(&deferred_coin_id(Pulse(9), 0)));
    }

    #[test]
    fn treasury_totals_and_returned_amounts_saturate_but_unallocated_does_not() {
        let usd = AssetId::usd();
        let mut treasury = Treasury::default();
        treasury.balances.insert(usd, Amount(u128::MAX - 1));
        treasury.minted.insert(usd, Amount(u128::MAX - 2));
        let bounded_id = ObjectId::derive(b"bounded");
        let bounded = |unallocated: u128| {
            shared(
                bounded_id,
                ObjectData::Bounded(BoundedTotal {
                    label: "b".into(),
                    asset: usd,
                    manager: AccountId::derive(b"m"),
                    total: Amount(100),
                    unallocated: Amount(unallocated),
                    allotments: BTreeMap::new(),
                    returned: Amount(u128::MAX - 3),
                }),
            )
        };
        let items = [
            Deferred::Treasury {
                asset: usd,
                amount: Amount(5),
            },
            Deferred::Minted {
                asset: usd,
                amount: Amount(5),
            },
            Deferred::ReturnToBounded {
                bounded: bounded_id,
                amount: Amount(5),
            },
        ];
        let ctx = context(3);
        let objects = vec![
            schedule(BTreeMap::new()),
            shared(
                well_known::treasury(),
                ObjectData::Treasury(treasury.clone()),
            ),
            bounded(10),
        ];
        let outcome =
            run_deferred_phase(&ctx, &view(objects), Frontier::default(), &items, &[]).unwrap();
        let ObjectData::Treasury(after) = &outcome.writes[&well_known::treasury()].data else {
            panic!("treasury")
        };
        assert_eq!(after.balances[&usd], Amount(u128::MAX));
        assert_eq!(after.minted[&usd], Amount(u128::MAX));
        let ObjectData::Bounded(after) = &outcome.writes[&bounded_id].data else {
            panic!("bounded")
        };
        assert_eq!(after.returned, Amount(u128::MAX));
        assert_eq!(after.unallocated, Amount(15));
        // `unallocated += amount` stays checked: its overflow invalidates
        // the phase.
        let objects = vec![
            schedule(BTreeMap::new()),
            shared(well_known::treasury(), ObjectData::Treasury(treasury)),
            bounded(u128::MAX - 1),
        ];
        assert!(matches!(
            run_deferred_phase(&ctx, &view(objects), Frontier::default(), &items, &[]),
            Err(SpecError::DeferredPhase(_))
        ));
    }

    #[test]
    fn sealing_times_saturate() {
        // An order at the last Pulses: (index + 1) × length saturates, so
        // the batch seals at u64::MAX − 1, in this very phase; its deadline
        // saturates at u64::MAX.
        let ctx = context(u64::MAX - 1);
        let order = pending(b"late");
        let items = [Deferred::AddOrder {
            market: ObjectId::derive(b"market"),
            order: order.clone(),
        }];
        let objects = vec![
            schedule(BTreeMap::new()),
            market(MAX_PERIOD, MAX_PERIOD, Vec::new()),
        ];
        let outcome =
            run_deferred_phase(&ctx, &view(objects), Frontier::default(), &items, &[]).unwrap();
        let state = market_of(&outcome);
        let index = (u64::MAX - 1) / MAX_PERIOD;
        let batch = &state.batches[&index];
        assert_eq!(
            batch.status,
            BatchStatus::Sealed {
                beacon: Hash32::ZERO,
                at: Pulse(u64::MAX - 1)
            }
        );
        assert_eq!(batch.orders, vec![order]);
        let ObjectData::Schedule(after) = &outcome.writes[&well_known::schedule()].data else {
            panic!("schedule")
        };
        let deadline = ScheduledEvent::BatchDeadline {
            market: ObjectId::derive(b"market"),
            batch: index,
        };
        assert_eq!(
            after.due,
            [(u64::MAX, [deadline].into_iter().collect())]
                .into_iter()
                .collect()
        );
    }

    #[test]
    fn events_scheduled_while_processing_due_events_wait() {
        // At Pulse u64::MAX a sealing schedules its deadline at the same
        // Pulse (saturating): it is not processed in this Pulse.
        let ctx = context(u64::MAX);
        let market_id = ObjectId::derive(b"market");
        let seal = ScheduledEvent::SealBatch {
            market: market_id,
            batch: 4,
        };
        let open = MarketBatch {
            index: 4,
            orders: vec![pending(b"waiting")],
            status: BatchStatus::Open,
        };
        let objects = vec![
            schedule(
                [(u64::MAX, [seal].into_iter().collect())]
                    .into_iter()
                    .collect(),
            ),
            market(2, 3, vec![open]),
        ];
        let outcome =
            run_deferred_phase(&ctx, &view(objects), Frontier::default(), &[], &[]).unwrap();
        let state = market_of(&outcome);
        assert!(matches!(
            state.batches[&4].status,
            BatchStatus::Sealed { .. }
        ));
        assert!(
            !outcome
                .writes
                .contains_key(&deferred_coin_id(Pulse(u64::MAX), 0)),
            "not refunded"
        );
        let ObjectData::Schedule(after) = &outcome.writes[&well_known::schedule()].data else {
            panic!("schedule")
        };
        let deadline = ScheduledEvent::BatchDeadline {
            market: market_id,
            batch: 4,
        };
        assert_eq!(after.due[&u64::MAX], [deadline].into_iter().collect());
    }

    #[test]
    fn items_sort_by_rank_then_natural_key() {
        let usd = AssetId::usd();
        let items = vec![
            Deferred::Schedule {
                at: Pulse(3),
                event: ScheduledEvent::RegistryTick,
            },
            Deferred::Minted {
                asset: usd,
                amount: Amount(1),
            },
            Deferred::Treasury {
                asset: usd,
                amount: Amount(2),
            },
            Deferred::Treasury {
                asset: usd,
                amount: Amount(1),
            },
            Deferred::RegisterAtom {
                atom: Atom::new(AtomClass::Code, "z"),
            },
        ];
        let mut sorted = items.clone();
        sorted.sort_by_key(sort_key);
        let ranks: Vec<u8> = sorted.iter().map(|item| sort_key(item).0).collect();
        assert_eq!(ranks, vec![0, 5, 5, 6, 7]);
        // Equal natural keys fall back to the item encoding.
        assert_eq!(
            sorted[1],
            Deferred::Treasury {
                asset: usd,
                amount: Amount(1)
            }
        );
    }
}

#[cfg(test)]
mod registry_tick_tests {
    use super::*;
    use std::collections::BTreeSet;

    use sion_assurance::{accept_and_reserve, Atom, AtomClass, Formula, Policy, Registry};
    use sion_core::ids::{ClaimId, DomainId};
    use sion_exec::state::ChainConfig;

    fn shared(id: ObjectId, data: ObjectData) -> Object {
        Object {
            id,
            version: Version(1),
            owner: Owner::Shared(DomainId::derive(b"test")),
            data,
        }
    }

    #[test]
    fn registry_ticks_saturate_the_next_pulse_and_the_treasury_credit() {
        // A registry holding a slashing pool that a reservation still needs:
        // OR(a, b) is relied on, `a` breaks, the reservation degrades and
        // `a`'s pool keeps what it may still owe until the reliance ends.
        let owner = AccountId::derive(b"owner");
        let mut registry = Registry::new();
        let a = registry
            .register_atom(Atom::bonded(AtomClass::Economic, "a"))
            .unwrap();
        let b = registry
            .register_atom(Atom::bonded(AtomClass::Economic, "b"))
            .unwrap();
        registry
            .post_bond(a, owner, Amount(1_000), Pulse(0))
            .unwrap();
        registry
            .post_bond(b, owner, Amount(1_000), Pulse(0))
            .unwrap();
        let claim = ClaimId::derive(b"either");
        registry
            .issue_claim(ClaimRecord {
                id: claim,
                issuer: owner,
                formula: Formula::or(vec![Formula::atom(a), Formula::atom(b)]),
                adjudicator: None,
                detection: Ratio::ONE,
                issued_at: Pulse(0),
                valid_until: Pulse(100),
                challenge_until: Pulse(100),
                status: ClaimStatus::Active,
                sequence: 0,
            })
            .unwrap();
        let (_, reservation) = accept_and_reserve(
            &mut registry,
            &Policy::paranoid(),
            AccountId::derive(b"consumer"),
            &claim,
            Amount(100),
            Pulse(10),
            0,
        )
        .unwrap();
        let reservation = reservation.expect("reserved");
        let allocated = registry
            .reservation(&reservation)
            .unwrap()
            .allocations
            .clone();
        let broken = *allocated
            .iter()
            .find(|(_, amount)| **amount > Amount::ZERO)
            .expect("an allocation")
            .0;
        registry.declare_broken(broken, Pulse(1)).unwrap();
        let pool = registry.bond(&broken).unwrap().pool;
        assert!(pool > Amount::ZERO, "the pool still backs the reservation");

        // The tick at the last Pulse: `ctx.pulse + 1` saturates, the
        // reservation ends, the pool is released into a nearly full treasury
        // (saturating).
        let usd = AssetId::usd();
        let mut treasury = Treasury::default();
        treasury.balances.insert(usd, Amount(u128::MAX - 1));
        let tick: BTreeSet<ScheduledEvent> = [ScheduledEvent::RegistryTick].into_iter().collect();
        let objects = vec![
            shared(
                well_known::schedule(),
                ObjectData::Schedule(Schedule {
                    due: [(u64::MAX, tick)].into_iter().collect(),
                }),
            ),
            shared(
                well_known::registry(),
                ObjectData::Assurance(Box::new(AssuranceDomain {
                    registry,
                    vault: Amount(1_000_000),
                    deposits: BTreeMap::new(),
                })),
            ),
            shared(well_known::treasury(), ObjectData::Treasury(treasury)),
        ];
        let view = ChunkView::new(objects.into_iter().map(|o| (o.id, Some(o))).collect());
        let ctx = SpecContext {
            pulse: Pulse(u64::MAX),
            beacon: Hash32::ZERO,
            config: ChainConfig {
                chain_id: "test".into(),
                shard_count: 1,
                base_fee: Amount(1_000),
                fee_per_kib: Amount(100),
                faucet: None,
                max_claim_window: 1_000,
                max_formula_size: 64,
                max_payload_bytes: 1_024,
                challenger_share_permille: 500,
            },
        };
        let outcome = run_deferred_phase(&ctx, &view, Frontier::default(), &[], &[]).unwrap();
        let ObjectData::Assurance(domain) = &outcome.writes[&well_known::registry()].data else {
            panic!("assurance")
        };
        assert_eq!(domain.registry.pulse(), Pulse(u64::MAX));
        assert_eq!(domain.registry.bond(&broken).unwrap().pool, Amount::ZERO);
        assert_eq!(domain.vault, Amount(1_000_000 - pool.0));
        let ObjectData::Treasury(after) = &outcome.writes[&well_known::treasury()].data else {
            panic!("treasury")
        };
        assert_eq!(after.balances[&usd], Amount(u128::MAX));
    }
}
