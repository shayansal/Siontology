//! One transaction's bookkeeping (spec §3): the scratch view plus
//! `next_index`, `fee_paid` and the effects under construction, with
//! `create`, `mint`, `draw`, `charge_fee_from_gas` and `finalize_inputs`.

use std::collections::BTreeSet;

use sion_core::ids::{AccountId, AssetId, ObjectId, TxDigest, Version};
use sion_core::Amount;
use sion_exec::execute::{Deferred, Event, ExecStatus, TxEffects};
use sion_exec::object::{created_id, Coin, Object, ObjectData, Owner};
use sion_exec::tx::{SignedTransaction, Transaction};

use crate::context::SpecContext;
use crate::error::{fail, Step};
use crate::journal::{Scratch, ScratchParts};
use crate::view::ChunkView;

/// The USD asset.
pub(crate) fn usd() -> AssetId {
    AssetId::usd()
}

/// One run of one transaction: fresh bookkeeping over the chunk view.
#[derive(Debug)]
pub(crate) struct TxRun<'v> {
    /// The Pulse context.
    pub ctx: &'v SpecContext,
    /// The signed transaction.
    pub signed: &'v SignedTransaction,
    /// Its body.
    pub tx: &'v Transaction,
    /// `digest = SignedTransaction::digest()`.
    pub digest: TxDigest,
    /// Loads, stores and the created / mutated / consumed sets.
    pub book: Scratch<'v>,
    /// Index of the next object `create` makes.
    pub next_index: u32,
    /// Whether the fee has been charged.
    pub fee_paid: bool,
    /// `effects.fee`.
    pub fee: Amount,
    /// `effects.events`.
    pub events: Vec<Event>,
    /// `effects.deferred`.
    pub deferred: Vec<Deferred>,
}

/// What a finished run leaves behind.
#[derive(Debug)]
pub(crate) struct RunOutcome {
    /// The scratch writes and bookkeeping.
    pub parts: ScratchParts,
    /// `effects.fee`.
    pub fee: Amount,
    /// `effects.events`.
    pub events: Vec<Event>,
    /// `effects.deferred`.
    pub deferred: Vec<Deferred>,
}

impl RunOutcome {
    /// The effects of this run with `status`.
    pub(crate) fn effects(
        &self,
        signed: &SignedTransaction,
        digest: TxDigest,
        status: ExecStatus,
    ) -> TxEffects {
        TxEffects {
            digest,
            sender: signed.tx.sender,
            class: signed.tx.class(),
            status,
            fee: self.fee,
            created: self.parts.created.clone(),
            mutated: self.parts.mutated.clone(),
            consumed: self.parts.consumed.clone(),
            events: self.events.clone(),
            deferred: self.deferred.clone(),
        }
    }
}

/// The effects of a rejected transaction: nothing but its identity.
pub(crate) fn rejected_effects(
    signed: &SignedTransaction,
    digest: TxDigest,
    reason: String,
) -> TxEffects {
    TxEffects {
        digest,
        sender: signed.tx.sender,
        class: signed.tx.class(),
        status: ExecStatus::Rejected(reason),
        fee: Amount::ZERO,
        created: BTreeSet::new(),
        mutated: BTreeSet::new(),
        consumed: BTreeSet::new(),
        events: Vec::new(),
        deferred: Vec::new(),
    }
}

impl<'v> TxRun<'v> {
    /// Fresh bookkeeping for `signed` over `view`.
    pub(crate) fn new(
        ctx: &'v SpecContext,
        view: &'v ChunkView,
        signed: &'v SignedTransaction,
    ) -> Self {
        TxRun {
            ctx,
            signed,
            tx: &signed.tx,
            digest: signed.digest(),
            book: Scratch::new(view),
            next_index: 0,
            fee_paid: false,
            fee: Amount::ZERO,
            events: Vec::new(),
            deferred: Vec::new(),
        }
    }

    /// The sender.
    pub(crate) fn sender(&self) -> AccountId {
        self.tx.sender
    }

    /// Ends the run.
    pub(crate) fn finish(self) -> RunOutcome {
        RunOutcome {
            parts: self.book.finish(),
            fee: self.fee,
            events: self.events,
            deferred: self.deferred,
        }
    }

    /// **load(id)** (§3.1).
    pub(crate) fn load(&mut self, id: &ObjectId) -> Step<Object> {
        self.book.load(id)
    }

    /// **try_load(id)** (§3.1).
    pub(crate) fn try_load(&mut self, id: &ObjectId) -> Step<Option<Object>> {
        self.book.try_load(id)
    }

    /// **store(object)** (§3.1).
    pub(crate) fn store(&mut self, object: Object) -> Step {
        self.book.store(object)
    }

    /// **consume(id)** (§3.1): tombstones the object as consumed by this
    /// transaction at this Pulse.
    pub(crate) fn consume(&mut self, id: &ObjectId) -> Step<Object> {
        self.book.consume(id, self.digest, self.ctx.pulse)
    }

    /// The identifier the next `create` will use.
    pub(crate) fn next_created_id(&self) -> ObjectId {
        created_id(&self.digest, self.next_index)
    }

    /// **create(owner, data)** (§3.1): stores a new object under
    /// `created_id(digest, next_index)` and returns its identifier.
    pub(crate) fn create(&mut self, owner: Owner, data: ObjectData) -> Step<ObjectId> {
        let id = self.next_created_id();
        self.next_index = self
            .next_index
            .checked_add(1)
            .ok_or_else(|| fail("too many created objects"))?;
        self.store(Object {
            id,
            version: Version(0),
            owner,
            data,
        })?;
        Ok(id)
    }

    /// **mint(owner, asset, amount)** (§3.1): a new coin unless `amount = 0`.
    pub(crate) fn mint(&mut self, owner: AccountId, asset: AssetId, amount: Amount) -> Step {
        if amount == Amount::ZERO {
            return Ok(());
        }
        self.create(
            Owner::Account(owner),
            ObjectData::Coin(Coin { asset, amount }),
        )?;
        Ok(())
    }

    /// "event E" (§5).
    pub(crate) fn event(&mut self, event: Event) {
        self.events.push(event);
    }

    /// "defer D" (§5).
    pub(crate) fn defer(&mut self, item: Deferred) {
        self.deferred.push(item);
    }

    /// Records the fee as charged.
    fn record_fee(&mut self, fee: Amount) {
        self.fee_paid = true;
        self.fee = fee;
        self.defer(Deferred::Treasury {
            asset: usd(),
            amount: fee,
        });
    }

    /// **draw(asset, amount)** (§3.2): consumes every input coin of `asset`
    /// and returns the change, paying the fee from USD coins if it is not yet
    /// paid.
    pub(crate) fn draw(&mut self, asset: AssetId, amount: Amount) -> Step {
        let mut available = Amount::ZERO;
        let mut used = Vec::new();
        for input in &self.tx.inputs {
            let object = self.load(&input.id)?;
            if let Some(coin) = object.as_coin() {
                if coin.asset == asset {
                    available = available.checked_add(coin.amount)?;
                    used.push(input.id);
                }
            }
        }
        let fee = if asset == usd() && !self.fee_paid {
            self.tx.fee
        } else {
            Amount::ZERO
        };
        let need = amount.checked_add(fee)?;
        if available < need {
            return Err(fail(format!(
                "insufficient funds: need {need}, have {available}"
            )));
        }
        for id in &used {
            self.consume(id)?;
        }
        if fee > Amount::ZERO {
            self.record_fee(fee);
        }
        let change = available.checked_sub(need)?;
        self.mint(self.tx.sender, asset, change)
    }

    /// **charge_fee_from_gas** (§3.3): if the fee is still unpaid, takes it
    /// from the gas coin `inputs[0]`.
    pub(crate) fn charge_fee_from_gas(&mut self) -> Step {
        let fee = self.tx.fee;
        if self.fee_paid || fee == Amount::ZERO {
            return Ok(());
        }
        let gas = self
            .tx
            .inputs
            .first()
            .ok_or_else(|| fail("no gas input"))?
            .id;
        let mut object = self.load(&gas)?;
        if object.is_tombstone() {
            return Err(fail("the gas coin was consumed"));
        }
        let ObjectData::Coin(coin) = &mut object.data else {
            return Err(fail("the gas input is not a coin"));
        };
        coin.amount = coin
            .amount
            .checked_sub(fee)
            .map_err(|_| fail("the gas coin cannot pay the fee"))?;
        self.store(object)?;
        self.record_fee(fee);
        Ok(())
    }

    /// **finalize_inputs** (§3.3): stores unchanged every input this
    /// transaction did not store, bumping its version (releasing its lock).
    pub(crate) fn finalize_inputs(&mut self) -> Step {
        for input in &self.tx.inputs {
            let object = self.load(&input.id)?;
            if object.version == input.version {
                self.store(object)?;
            }
        }
        Ok(())
    }
}
