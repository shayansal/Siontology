//! Executing one transaction.
//!
//! A transaction first passes preparation (not expired, fee sufficient,
//! inputs present, owned by the sender and at the expected versions,
//! authorization valid). A transaction that fails preparation is
//! *rejected*: it changes nothing, because it should never have been
//! certified. A prepared transaction then runs its operation in a journaled
//! scope; if the operation fails, its writes are undone, the fee is still
//! charged and every owned input's version is bumped (so the inputs' locks
//! are released), and the transaction is recorded as *failed*.

use std::collections::{BTreeMap, BTreeSet};

use borsh::{BorshDeserialize, BorshSerialize};
use fold_assurance::{
    accept_and_reserve, decide, release, Atom, AtomClass, ClaimStatus, Formula, SlashOutcome,
};
use fold_clearing::{verify_clearing, Side};
use fold_core::encode::{decode, encode};
use fold_core::hash::{hash_parts, Hash32};
use fold_core::ids::{AccountId, AssetId, AtomId, ClaimId, ObjectId, Pulse, TxDigest, Version};
use fold_core::Amount;
use serde::{Deserialize, Serialize};

use crate::account::AccountState;
use crate::assurance::AssuranceDomain;
use crate::charter::{Allotment, Caveats, Charter};
use crate::claim::{claim_id, formulas, ClaimBody, Payload};
use crate::error::ExecError;
use crate::escrow::BoundedTotal;
use crate::market::{
    sealing_aad, BatchStatus, Market, OrderTerms, PendingOrder, Submission, SEALED_ORDER_DOMAIN,
};
use crate::object::{
    created_id, well_known, Coin, CoordinationClass, Object, ObjectData, Owner, ScheduledEvent,
    Tombstone,
};
use crate::state::ChainConfig;
use crate::store::{ObjectStore, Overlay};
use crate::tx::{object_domain, SignedTransaction, Transaction, TxKind, TX_DOMAIN};

/// Context shared by every transaction in a Pulse.
#[derive(Clone, Debug)]
pub struct PulseContext {
    /// The Pulse being executed.
    pub pulse: Pulse,
    /// The Pulse's beacon value.
    pub beacon: Hash32,
    /// Chain configuration.
    pub config: ChainConfig,
    /// Whether to verify authorizations (validators already did at approval;
    /// provers and auditors always do).
    pub verify_signatures: bool,
}

/// How a transaction ended.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub enum ExecStatus {
    /// Applied.
    Success,
    /// The operation failed; the fee was charged and inputs released.
    Failed(String),
    /// Invalid before execution; nothing changed.
    Rejected(String),
}

/// Effects applied after all transactions of a Pulse, in canonical order.
/// They commute, so they need no ordering among themselves.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub enum Deferred {
    /// Append a claim to the knowledge log.
    IssueClaim {
        /// Issuer.
        issuer: AccountId,
        /// The account whose bonded atoms the claim may rest on directly:
        /// the issuer, or for an agent's receipt the principal who bonded
        /// the agent.
        backer: AccountId,
        /// Body.
        body: ClaimBody,
    },
    /// Register an atom.
    RegisterAtom {
        /// The atom.
        atom: Atom,
    },
    /// Post a bond (the funds are already drawn from the sender).
    PostBond {
        /// The bonded atom.
        atom: Atom,
        /// Owner.
        owner: AccountId,
        /// Amount.
        amount: Amount,
    },
    /// Add an order to a market's open batch.
    AddOrder {
        /// The market.
        market: ObjectId,
        /// The order.
        order: PendingOrder,
    },
    /// Return an allotment's remainder to its bounded total.
    ReturnToBounded {
        /// The bounded total.
        bounded: ObjectId,
        /// Amount.
        amount: Amount,
    },
    /// Credit the treasury.
    Treasury {
        /// Asset.
        asset: AssetId,
        /// Amount.
        amount: Amount,
    },
    /// Record newly minted supply.
    Minted {
        /// Asset.
        asset: AssetId,
        /// Amount.
        amount: Amount,
    },
    /// Schedule a protocol event.
    Schedule {
        /// The Pulse at whose end it fires.
        at: Pulse,
        /// The event.
        event: ScheduledEvent,
    },
}

/// Something that happened, for receipts and the API.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub enum Event {
    /// Coins moved.
    Transferred {
        /// Asset.
        asset: AssetId,
        /// Recipients.
        outputs: Vec<(AccountId, Amount)>,
    },
    /// The faucet minted coins.
    Minted {
        /// Recipient.
        to: AccountId,
        /// Asset.
        asset: AssetId,
        /// Amount.
        amount: Amount,
    },
    /// A policy rotated.
    PolicyRotated {
        /// Account.
        account: AccountId,
    },
    /// A claim was submitted (it joins the knowledge log at Pulse end).
    ClaimSubmitted {
        /// Claim.
        claim: ClaimId,
    },
    /// A claim joined the knowledge log.
    ClaimIssued {
        /// Claim.
        claim: ClaimId,
        /// Index in the log.
        log_index: u64,
    },
    /// A submitted claim was invalid.
    ClaimRejected {
        /// Claim.
        claim: ClaimId,
        /// Why.
        reason: String,
    },
    /// An atom was registered.
    AtomRegistered {
        /// Atom.
        atom: AtomId,
    },
    /// A bond was posted.
    BondPosted {
        /// Atom.
        atom: AtomId,
        /// Owner.
        owner: AccountId,
        /// Amount.
        amount: Amount,
    },
    /// A reservation was made.
    Reserved {
        /// Reservation.
        reservation: Hash32,
        /// Claim.
        claim: ClaimId,
        /// κ·V reserved.
        amount: Amount,
    },
    /// A reservation was released.
    Released {
        /// Reservation.
        reservation: Hash32,
    },
    /// A challenge was opened.
    ChallengeOpened {
        /// Challenge.
        challenge: Hash32,
        /// Claim.
        claim: ClaimId,
        /// Target atom.
        target: AtomId,
    },
    /// A challenge was decided.
    ChallengeRuled {
        /// Challenge.
        challenge: Hash32,
        /// Outcome.
        upheld: bool,
    },
    /// A bond was slashed.
    Slashed {
        /// Atom.
        atom: AtomId,
        /// Amount.
        amount: Amount,
    },
    /// A claim was falsified.
    Falsified {
        /// Claim.
        claim: ClaimId,
        /// Broken atoms beneath it.
        causes: Vec<AtomId>,
    },
    /// A consumer was compensated.
    Compensated {
        /// Reservation.
        reservation: Hash32,
        /// Consumer.
        consumer: AccountId,
        /// Amount.
        amount: Amount,
    },
    /// A bond was withdrawn.
    BondWithdrawn {
        /// Atom.
        atom: AtomId,
        /// Amount.
        amount: Amount,
    },
    /// A charter was created.
    CharterCreated {
        /// Charter.
        charter: ObjectId,
        /// Agent's allotment.
        allotment: ObjectId,
    },
    /// A charter was revoked.
    CharterRevoked {
        /// Charter.
        charter: ObjectId,
    },
    /// An agent paid.
    AgentPaid {
        /// Charter.
        charter: ObjectId,
        /// Payee.
        payee: AccountId,
        /// Amount.
        amount: Amount,
        /// Receipt claim, if receipts are on.
        receipt: Option<ClaimId>,
    },
    /// A bounded total was created.
    BoundedCreated {
        /// The total.
        bounded: ObjectId,
        /// Allotments created.
        allotments: Vec<ObjectId>,
    },
    /// An allotment was spent or returned.
    AllotmentUsed {
        /// Allotment.
        allotment: ObjectId,
        /// Amount.
        amount: Amount,
    },
    /// A market was created.
    MarketCreated {
        /// Market.
        market: ObjectId,
    },
    /// An order was submitted.
    OrderSubmitted {
        /// Market.
        market: ObjectId,
        /// Order.
        order: Hash32,
    },
    /// A batch sealed.
    BatchSealed {
        /// Market.
        market: ObjectId,
        /// Batch.
        batch: u64,
        /// Orders.
        orders: u64,
    },
    /// Sealed orders were revealed.
    BatchRevealed {
        /// Market.
        market: ObjectId,
        /// Batch.
        batch: u64,
        /// Orders revealed.
        revealed: u64,
    },
    /// A batch cleared with a certified-optimal allocation.
    BatchCleared {
        /// Market.
        market: ObjectId,
        /// Batch.
        batch: u64,
        /// Uniform price.
        price: Option<Amount>,
        /// Units traded.
        volume: u64,
        /// Gains from trade.
        welfare: Amount,
    },
    /// A batch was refunded.
    BatchRefunded {
        /// Market.
        market: ObjectId,
        /// Batch.
        batch: u64,
    },
}

/// The effects of one transaction.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct TxEffects {
    /// Transaction.
    pub digest: TxDigest,
    /// Sender.
    pub sender: AccountId,
    /// Derived coordination class.
    pub class: CoordinationClass,
    /// Outcome.
    pub status: ExecStatus,
    /// Fee charged.
    pub fee: Amount,
    /// Objects created.
    pub created: BTreeSet<ObjectId>,
    /// Objects mutated.
    pub mutated: BTreeSet<ObjectId>,
    /// Objects consumed.
    pub consumed: BTreeSet<ObjectId>,
    /// Events.
    pub events: Vec<Event>,
    /// Deferred effects for the end of the Pulse.
    pub deferred: Vec<Deferred>,
}

impl TxEffects {
    fn new(tx: &Transaction, digest: TxDigest) -> Self {
        TxEffects {
            digest,
            sender: tx.sender,
            class: tx.class(),
            status: ExecStatus::Success,
            fee: Amount::ZERO,
            created: BTreeSet::new(),
            mutated: BTreeSet::new(),
            consumed: BTreeSet::new(),
            events: Vec::new(),
            deferred: Vec::new(),
        }
    }

    fn clear(&mut self) {
        self.fee = Amount::ZERO;
        self.created.clear();
        self.mutated.clear();
        self.consumed.clear();
        self.events.clear();
        self.deferred.clear();
    }

    /// Whether the transaction succeeded.
    pub fn succeeded(&self) -> bool {
        self.status == ExecStatus::Success
    }
}

struct Run<'r, 'a, S: ObjectStore> {
    overlay: &'r mut Overlay<'a, S>,
    ctx: &'r PulseContext,
    tx: &'r Transaction,
    digest: TxDigest,
    originals: BTreeMap<ObjectId, Version>,
    created: BTreeSet<ObjectId>,
    next_index: u32,
    fee_paid: bool,
    effects: TxEffects,
}

impl<'r, 'a, S: ObjectStore> Run<'r, 'a, S> {
    fn load(&mut self, id: &ObjectId) -> Result<Object, ExecError> {
        let object = self.overlay.require(id)?;
        if !self.created.contains(id) {
            self.originals.entry(*id).or_insert(object.version);
        }
        Ok(object)
    }

    fn try_load(&mut self, id: &ObjectId) -> Result<Option<Object>, ExecError> {
        let object = self.overlay.get(id)?;
        if let Some(object) = &object {
            if !self.created.contains(id) {
                self.originals.entry(*id).or_insert(object.version);
            }
        }
        Ok(object)
    }

    fn store(&mut self, mut object: Object) {
        if self.created.contains(&object.id) {
            object.version = Version(0);
        } else if let Some(version) = self.originals.get(&object.id) {
            object.version = version.next();
            if !object.is_tombstone() {
                self.effects.mutated.insert(object.id);
            }
        } else {
            object.version = Version(0);
            self.created.insert(object.id);
            self.effects.created.insert(object.id);
        }
        self.overlay.put(object);
    }

    fn create(&mut self, owner: Owner, data: ObjectData) -> ObjectId {
        let id = created_id(&self.digest, self.next_index);
        self.next_index += 1;
        self.store(Object {
            id,
            version: Version(0),
            owner,
            data,
        });
        id
    }

    fn mint_coin(&mut self, owner: AccountId, asset: AssetId, amount: Amount) -> Option<ObjectId> {
        if amount == Amount::ZERO {
            return None;
        }
        Some(self.create(
            Owner::Account(owner),
            ObjectData::Coin(Coin { asset, amount }),
        ))
    }

    fn consume(&mut self, id: &ObjectId) -> Result<Object, ExecError> {
        let object = self.load(id)?;
        let tombstone = Object {
            id: *id,
            version: object.version,
            owner: object.owner,
            data: ObjectData::Tombstone(Tombstone {
                consumed_by: self.digest,
                at: self.ctx.pulse,
            }),
        };
        self.store(tombstone);
        self.effects.mutated.remove(id);
        self.effects.consumed.insert(*id);
        Ok(object)
    }

    /// Consumes every input coin of `asset` and returns change. When the
    /// asset is USD, the fee is paid from the same coins.
    fn draw(&mut self, asset: AssetId, amount: Amount) -> Result<(), ExecError> {
        let mut available = Amount::ZERO;
        let mut used = Vec::new();
        for input in self.tx.inputs.clone() {
            let object = self.load(&input.id)?;
            if let ObjectData::Coin(coin) = &object.data {
                if coin.asset == asset {
                    available = available.checked_add(coin.amount)?;
                    used.push(input.id);
                }
            }
        }
        let fee = if asset == AssetId::usd() && !self.fee_paid {
            self.tx.fee
        } else {
            Amount::ZERO
        };
        let need = amount.checked_add(fee)?;
        if available < need {
            return Err(ExecError::InsufficientFunds {
                need: need.to_string(),
                have: available.to_string(),
            });
        }
        for id in &used {
            self.consume(id)?;
        }
        if fee > Amount::ZERO {
            self.fee_paid = true;
            self.effects.fee = fee;
            self.effects.deferred.push(Deferred::Treasury {
                asset: AssetId::usd(),
                amount: fee,
            });
        }
        let change = available.checked_sub(need)?;
        self.mint_coin(self.tx.sender, asset, change);
        Ok(())
    }

    fn charge_fee_from_gas(&mut self) -> Result<(), ExecError> {
        if self.fee_paid || self.tx.fee == Amount::ZERO {
            return Ok(());
        }
        let gas_id = self.tx.inputs[0].id;
        let mut gas = self.load(&gas_id)?;
        if gas.is_tombstone() {
            return Err(ExecError::Invalid("gas coin already consumed".into()));
        }
        let ObjectData::Coin(coin) = &mut gas.data else {
            return Err(ExecError::Invalid("gas input is not a coin".into()));
        };
        coin.amount =
            coin.amount
                .checked_sub(self.tx.fee)
                .map_err(|_| ExecError::InsufficientFunds {
                    need: self.tx.fee.to_string(),
                    have: coin.amount.to_string(),
                })?;
        self.store(gas);
        self.fee_paid = true;
        self.effects.fee = self.tx.fee;
        self.effects.deferred.push(Deferred::Treasury {
            asset: AssetId::usd(),
            amount: self.tx.fee,
        });
        Ok(())
    }

    /// Bumps the version of every owned input not otherwise written.
    fn finalize_inputs(&mut self) -> Result<(), ExecError> {
        for input in self.tx.inputs.clone() {
            let object = self.load(&input.id)?;
            if object.version == input.version {
                self.store(object);
            }
        }
        Ok(())
    }

    fn input(&self, index: usize) -> Result<ObjectId, ExecError> {
        self.tx
            .inputs
            .get(index)
            .map(|r| r.id)
            .ok_or_else(|| ExecError::Invalid(format!("missing input {index}")))
    }

    fn assurance(&mut self) -> Result<(Object, AssuranceDomain), ExecError> {
        let object = self.load(&well_known::registry())?;
        let ObjectData::Assurance(domain) = &object.data else {
            return Err(ExecError::Invalid("registry object corrupted".into()));
        };
        let domain = (**domain).clone();
        Ok((object, domain))
    }

    fn store_assurance(&mut self, mut object: Object, domain: AssuranceDomain) {
        object.data = ObjectData::Assurance(Box::new(domain));
        self.store(object);
    }

    fn event(&mut self, event: Event) {
        self.effects.events.push(event);
    }
}

/// Checks and authorizes a transaction before execution. Creates the
/// sender's account object on its first transaction.
fn prepare<S: ObjectStore>(
    run: &mut Run<'_, '_, S>,
    signed: &SignedTransaction,
) -> Result<(), ExecError> {
    let tx = run.tx;
    let ctx = run.ctx;
    if ctx.pulse > tx.expires_at {
        return Err(ExecError::Expired);
    }
    if tx.inputs.is_empty() {
        return Err(ExecError::Invalid(
            "at least one owned input is required".into(),
        ));
    }
    let unique: BTreeSet<ObjectId> = tx.inputs.iter().map(|r| r.id).collect();
    if unique.len() != tx.inputs.len() {
        return Err(ExecError::Invalid("duplicate inputs".into()));
    }
    let is_faucet = matches!(tx.kind, TxKind::Faucet { .. });
    if !is_faucet {
        let min = ctx.config.min_fee(tx.size());
        if tx.fee < min {
            return Err(ExecError::Invalid(format!(
                "fee {} below minimum {}",
                tx.fee, min
            )));
        }
    }

    // Authorization against the current policy.
    let account_id = well_known::account(&tx.sender);
    let policy = match run.try_load(&account_id)? {
        Some(object) => match object.data {
            ObjectData::Account(state) => state.policy,
            _ => return Err(ExecError::Invalid("account object corrupted".into())),
        },
        None => {
            let policy = tx.genesis_policy.clone().ok_or_else(|| {
                ExecError::Unauthorized("unknown account; genesis policy required".into())
            })?;
            policy.validate()?;
            if policy.account_id() != tx.sender {
                return Err(ExecError::Unauthorized(
                    "genesis policy does not hash to the sender".into(),
                ));
            }
            run.store(Object {
                id: account_id,
                version: Version(0),
                owner: Owner::Account(tx.sender),
                data: ObjectData::Account(AccountState {
                    id: tx.sender,
                    policy: policy.clone(),
                    viewing_key: None,
                    created_at: ctx.pulse,
                    rotations: 0,
                }),
            });
            policy
        }
    };
    if ctx.verify_signatures {
        policy
            .verify(TX_DOMAIN, &run.digest.0 .0, &signed.auth)
            .map_err(|e| ExecError::Unauthorized(e.to_string()))?;
    }

    // Owned inputs.
    for (index, input) in tx.inputs.iter().enumerate() {
        let object = run.load(&input.id)?;
        if object.version != input.version {
            return Err(ExecError::VersionMismatch {
                id: input.id,
                expected: input.version,
                actual: object.version,
            });
        }
        if object.owner != Owner::Account(tx.sender) {
            return Err(ExecError::NotOwner(input.id));
        }
        if object.is_tombstone() {
            return Err(ExecError::Invalid(format!(
                "input {} already consumed",
                input.id
            )));
        }
        if index == 0 {
            if is_faucet {
                if Some(tx.sender) != ctx.config.faucet || input.id != account_id {
                    return Err(ExecError::Unauthorized("only the faucet may mint".into()));
                }
            } else {
                match object.as_coin() {
                    Some(coin) if coin.asset == AssetId::usd() && coin.amount >= tx.fee => {}
                    _ => {
                        return Err(ExecError::Invalid(
                            "inputs[0] must be a USD coin covering the fee".into(),
                        ))
                    }
                }
            }
        }
    }
    Ok(())
}

/// Executes one transaction over the overlay.
pub fn execute_transaction<S: ObjectStore>(
    overlay: &mut Overlay<'_, S>,
    ctx: &PulseContext,
    signed: &SignedTransaction,
) -> TxEffects {
    let digest = signed.digest();
    let tx = &signed.tx;
    let mut effects = TxEffects::new(tx, digest);

    overlay.begin();
    let outcome = {
        let mut run = Run {
            overlay: &mut *overlay,
            ctx,
            tx,
            digest,
            originals: BTreeMap::new(),
            created: BTreeSet::new(),
            next_index: 0,
            fee_paid: false,
            effects: TxEffects::new(tx, digest),
        };
        match prepare(&mut run, signed) {
            Err(error) => Err((true, error)),
            Ok(()) => match apply(&mut run)
                .and_then(|_| run.charge_fee_from_gas())
                .and_then(|_| run.finalize_inputs())
            {
                Ok(()) => {
                    effects = run.effects;
                    Ok(())
                }
                Err(error) => Err((false, error)),
            },
        }
    };
    match outcome {
        Ok(()) => {
            overlay.commit();
            effects.status = ExecStatus::Success;
        }
        Err((true, error)) => {
            overlay.rollback();
            effects.clear();
            effects.status = ExecStatus::Rejected(error.to_string());
        }
        Err((false, error)) => {
            overlay.rollback();
            // Charge the fee and release the inputs.
            overlay.begin();
            let mut run = Run {
                overlay: &mut *overlay,
                ctx,
                tx,
                digest,
                originals: BTreeMap::new(),
                created: BTreeSet::new(),
                next_index: 0,
                fee_paid: false,
                effects: TxEffects::new(tx, digest),
            };
            let settled = prepare(&mut run, signed)
                .and_then(|_| run.charge_fee_from_gas())
                .and_then(|_| run.finalize_inputs());
            let mut settled_effects = run.effects;
            match settled {
                Ok(()) => {
                    overlay.commit();
                    settled_effects.events.clear();
                    settled_effects.status = ExecStatus::Failed(error.to_string());
                    effects = settled_effects;
                }
                Err(_) => {
                    overlay.rollback();
                    effects.clear();
                    effects.status = ExecStatus::Rejected(error.to_string());
                }
            }
        }
    }
    effects
}

fn apply<S: ObjectStore>(run: &mut Run<'_, '_, S>) -> Result<(), ExecError> {
    let tx = run.tx;
    let pulse = run.ctx.pulse;
    match &tx.kind {
        TxKind::Transfer { asset, outputs } => {
            if outputs.is_empty() || outputs.len() > 256 {
                return Err(ExecError::Invalid("between 1 and 256 outputs".into()));
            }
            if outputs.iter().any(|(_, amount)| *amount == Amount::ZERO) {
                return Err(ExecError::Invalid("zero output".into()));
            }
            let total = Amount::sum(outputs.iter().map(|(_, a)| a))?;
            run.draw(*asset, total)?;
            for (to, amount) in outputs {
                run.mint_coin(*to, *asset, *amount);
            }
            run.event(Event::Transferred {
                asset: *asset,
                outputs: outputs.clone(),
            });
        }
        TxKind::Faucet { grants } => {
            if grants.is_empty() || grants.len() > 256 {
                return Err(ExecError::Invalid("between 1 and 256 grants".into()));
            }
            for (to, asset, amount) in grants {
                if *amount == Amount::ZERO {
                    return Err(ExecError::Invalid("zero grant".into()));
                }
                run.mint_coin(*to, *asset, *amount);
                run.effects.deferred.push(Deferred::Minted {
                    asset: *asset,
                    amount: *amount,
                });
                run.event(Event::Minted {
                    to: *to,
                    asset: *asset,
                    amount: *amount,
                });
            }
        }
        TxKind::RotatePolicy {
            policy,
            viewing_key,
        } => {
            policy.validate()?;
            let account_id = well_known::account(&tx.sender);
            if !tx.inputs.iter().any(|r| r.id == account_id) {
                return Err(ExecError::Invalid(
                    "the account object must be an input".into(),
                ));
            }
            let mut account = run.load(&account_id)?;
            let ObjectData::Account(state) = &mut account.data else {
                return Err(ExecError::Invalid("account object corrupted".into()));
            };
            state.policy = policy.clone();
            state.viewing_key = viewing_key.clone();
            state.rotations += 1;
            run.store(account);
            run.event(Event::PolicyRotated { account: tx.sender });
        }
        TxKind::IssueClaim { claim } => {
            validate_claim_body(&run.ctx.config, claim)?;
            let id = claim_id(&tx.sender, claim);
            run.effects.deferred.push(Deferred::IssueClaim {
                issuer: tx.sender,
                backer: tx.sender,
                body: claim.clone(),
            });
            run.event(Event::ClaimSubmitted { claim: id });
        }
        TxKind::RegisterAtom { class, label } => {
            if !matches!(class, AtomClass::Code | AtomClass::Hardware) {
                return Err(ExecError::Invalid(
                    "only code and hardware atoms can be registered; others are bonded or set at genesis".into(),
                ));
            }
            if label.is_empty() || label.len() > 128 {
                return Err(ExecError::Invalid("label must be 1 to 128 bytes".into()));
            }
            // Registered atoms live in the registrant's namespace, so nobody
            // can squat a name governance will later register with a cost.
            run.effects.deferred.push(Deferred::RegisterAtom {
                atom: registered_atom(*class, &tx.sender, label),
            });
        }
        TxKind::PostBond {
            class,
            label,
            amount,
        } => {
            if !matches!(
                class,
                AtomClass::Economic | AtomClass::Institutional | AtomClass::Adjudicator
            ) {
                return Err(ExecError::Invalid(
                    "bonds back economic, institutional or adjudicator atoms".into(),
                ));
            }
            if *amount == Amount::ZERO {
                return Err(ExecError::Invalid("zero bond".into()));
            }
            run.draw(AssetId::usd(), *amount)?;
            let atom = bond_atom(*class, &tx.sender, label);
            run.effects.deferred.push(Deferred::PostBond {
                atom,
                owner: tx.sender,
                amount: *amount,
            });
        }
        TxKind::Accept {
            claim,
            value_at_risk,
            policy,
            until,
            nonce,
        } => {
            if *until <= pulse {
                return Err(ExecError::Invalid("reliance must end in the future".into()));
            }
            let (object, mut domain) = run.assurance()?;
            let (decision, reservation) = accept_and_reserve(
                &mut domain.registry,
                policy,
                tx.sender,
                claim,
                *value_at_risk,
                *until,
                *nonce,
            )?;
            let Some(reservation) = reservation else {
                return Err(ExecError::Invalid(format!(
                    "claim not accepted: {}",
                    decision.reasons.join("; ")
                )));
            };
            run.store_assurance(object, domain);
            run.effects.deferred.push(Deferred::Schedule {
                at: *until,
                event: ScheduledEvent::RegistryTick,
            });
            run.event(Event::Reserved {
                reservation,
                claim: *claim,
                amount: decision.required,
            });
        }
        TxKind::Release { reservation } => {
            let (object, mut domain) = run.assurance()?;
            release(&mut domain.registry, reservation, &tx.sender)?;
            run.store_assurance(object, domain);
            // Pool funds this reservation covered may now be free.
            run.effects.deferred.push(Deferred::Schedule {
                at: pulse,
                event: ScheduledEvent::RegistryTick,
            });
            run.event(Event::Released {
                reservation: *reservation,
            });
        }
        TxKind::Challenge {
            claim,
            target,
            evidence,
            deposit,
        } => {
            run.draw(AssetId::usd(), *deposit)?;
            let (object, mut domain) = run.assurance()?;
            let id = domain
                .registry
                .open_challenge(tx.sender, *claim, *target, *evidence, *deposit, pulse)?;
            domain.vault = domain.vault.checked_add(*deposit)?;
            domain.deposits.insert(id, (tx.sender, *deposit));
            run.store_assurance(object, domain);
            run.event(Event::ChallengeOpened {
                challenge: id,
                claim: *claim,
                target: *target,
            });
        }
        TxKind::Rule { challenge, upheld } => {
            let (object, mut domain) = run.assurance()?;
            let record = domain
                .registry
                .challenge(challenge)
                .cloned()
                .ok_or_else(|| ExecError::Invalid("unknown challenge".into()))?;
            let claim = domain
                .registry
                .claim(&record.claim)
                .cloned()
                .ok_or_else(|| ExecError::Invalid("unknown claim".into()))?;
            let adjudicator = claim
                .adjudicator
                .ok_or_else(|| ExecError::Invalid("claim has no adjudicator".into()))?;
            let owner = domain.registry.bond(&adjudicator).map(|b| b.owner);
            if owner != Some(tx.sender) {
                return Err(ExecError::Unauthorized(
                    "only the adjudicator's bond owner may rule".into(),
                ));
            }
            let outcome = domain
                .registry
                .rule(challenge, &adjudicator, *upheld, pulse)?;
            let (challenger, deposit) = domain
                .deposits
                .remove(challenge)
                .unwrap_or((record.challenger, Amount::ZERO));
            if *upheld {
                // Refund the deposit.
                domain.vault = domain.vault.checked_sub(deposit)?;
                run.mint_coin(challenger, AssetId::usd(), deposit);
            } else {
                // A frivolous challenge's deposit compensates the accused bond owner.
                domain.vault = domain.vault.checked_sub(deposit)?;
                let accused = domain
                    .registry
                    .bond(&record.target)
                    .map(|b| b.owner)
                    .unwrap_or(challenger);
                run.mint_coin(accused, AssetId::usd(), deposit);
            }
            if let Some(outcome) = outcome {
                settle_slash(run, &mut domain, outcome, Some(challenger))?;
            }
            run.store_assurance(object, domain);
            run.event(Event::ChallengeRuled {
                challenge: *challenge,
                upheld: *upheld,
            });
        }
        TxKind::WithdrawBond { atom } => {
            let (object, mut domain) = run.assurance()?;
            let amount = domain.registry.withdraw_bond(atom, &tx.sender, pulse)?;
            domain.vault = domain.vault.checked_sub(amount)?;
            run.mint_coin(tx.sender, AssetId::usd(), amount);
            run.store_assurance(object, domain);
            run.event(Event::BondWithdrawn {
                atom: *atom,
                amount,
            });
        }
        TxKind::CreateCharter {
            agent,
            caveats,
            funding,
            agent_bond,
        } => {
            if caveats.expires_at <= pulse {
                return Err(ExecError::Invalid(
                    "charter would already be expired".into(),
                ));
            }
            if caveats.window == 0 {
                return Err(ExecError::Invalid(
                    "window must be at least one Pulse".into(),
                ));
            }
            run.draw(AssetId::usd(), funding.checked_add(*agent_bond)?)?;
            let charter_id = created_id(&run.digest, run.next_index);
            let label = format!("charter:{charter_id}");
            let agent_atom = bond_atom(AtomClass::Economic, &tx.sender, &label);
            let domain = object_domain("charter", &charter_id);
            let created = run.create(
                Owner::Shared(domain),
                ObjectData::Charter(Charter {
                    principal: tx.sender,
                    agent: *agent,
                    caveats: caveats.clone(),
                    agent_atom: agent_atom.id,
                    backer: tx.sender,
                    revoked_at: None,
                    parent: None,
                    allotments: BTreeSet::new(),
                    created_at: pulse,
                }),
            );
            debug_assert_eq!(created, charter_id);
            let allotment = run.create(
                Owner::Account(*agent),
                ObjectData::Allotment(Allotment {
                    source: charter_id,
                    writer: *agent,
                    asset: AssetId::usd(),
                    remaining: *funding,
                    caveats: Some(caveats.clone()),
                    window_start: pulse,
                    spent_in_window: Amount::ZERO,
                }),
            );
            let mut charter = run.load(&charter_id)?;
            if let ObjectData::Charter(c) = &mut charter.data {
                c.allotments.insert(allotment);
            }
            run.store(charter);
            run.effects.deferred.push(Deferred::RegisterAtom {
                atom: agent_atom.clone(),
            });
            if *agent_bond > Amount::ZERO {
                run.effects.deferred.push(Deferred::PostBond {
                    atom: agent_atom,
                    owner: tx.sender,
                    amount: *agent_bond,
                });
            }
            run.event(Event::CharterCreated {
                charter: charter_id,
                allotment,
            });
        }
        TxKind::Delegate {
            charter,
            sub_agent,
            caveats,
            amount,
        } => {
            let allotment_id = run.input(1)?;
            let mut allotment = load_allotment(run, &allotment_id)?;
            let parent = load_charter(run, charter)?;
            check_charter_chain(run, charter)?;
            if allotment.source != *charter
                || allotment.writer != tx.sender
                || parent.agent != tx.sender
            {
                return Err(ExecError::Invalid(
                    "allotment does not belong to this charter's agent".into(),
                ));
            }
            let parent_caveats = allotment
                .caveats
                .clone()
                .ok_or_else(|| ExecError::Invalid("not a charter allotment".into()))?;
            if !parent_caveats.may_delegate {
                return Err(ExecError::Caveat("delegation not allowed".into()));
            }
            if !caveats.attenuates(&parent_caveats) {
                return Err(ExecError::Caveat(
                    "delegated caveats must be at least as strict".into(),
                ));
            }
            allotment.remaining = allotment.remaining.checked_sub(*amount).map_err(|_| {
                ExecError::InsufficientFunds {
                    need: amount.to_string(),
                    have: allotment.remaining.to_string(),
                }
            })?;
            store_allotment(run, allotment_id, allotment)?;
            let child_id = created_id(&run.digest, run.next_index);
            let child_atom = parent.agent_atom;
            run.create(
                Owner::Shared(object_domain("charter", &child_id)),
                ObjectData::Charter(Charter {
                    principal: tx.sender,
                    agent: *sub_agent,
                    caveats: caveats.clone(),
                    agent_atom: child_atom,
                    backer: parent.backer,
                    revoked_at: None,
                    parent: Some(*charter),
                    allotments: BTreeSet::new(),
                    created_at: pulse,
                }),
            );
            let child_allotment = run.create(
                Owner::Account(*sub_agent),
                ObjectData::Allotment(Allotment {
                    source: child_id,
                    writer: *sub_agent,
                    asset: AssetId::usd(),
                    remaining: *amount,
                    caveats: Some(caveats.clone()),
                    window_start: pulse,
                    spent_in_window: Amount::ZERO,
                }),
            );
            run.event(Event::CharterCreated {
                charter: child_id,
                allotment: child_allotment,
            });
        }
        TxKind::AgentPay {
            charter,
            payee,
            amount,
            purpose,
            evidence,
        } => {
            let allotment_id = run.input(1)?;
            let mut allotment = load_allotment(run, &allotment_id)?;
            let charter_state = load_charter(run, charter)?;
            check_charter_chain(run, charter)?;
            if allotment.source != *charter
                || allotment.writer != tx.sender
                || charter_state.agent != tx.sender
            {
                return Err(ExecError::Invalid(
                    "allotment does not belong to this charter's agent".into(),
                ));
            }
            let caveats = allotment
                .caveats
                .clone()
                .ok_or_else(|| ExecError::Invalid("not a charter allotment".into()))?;
            let evidence_ok = match (evidence, caveats.evidence_above) {
                (Some(claim), Some(threshold)) if *amount > threshold => {
                    Some(evidence_passes(run, &caveats, claim, *amount)?)
                }
                _ => None,
            };
            allotment.spend(payee, *amount, pulse, evidence_ok)?;
            store_allotment(run, allotment_id, allotment)?;
            run.mint_coin(*payee, AssetId::usd(), *amount);
            let receipt = if caveats.receipts {
                let body = ClaimBody {
                    schema: "receipt.agent.payment/v1".into(),
                    subject: format!("agent paid {amount} USD for: {purpose}"),
                    payload: Payload::Public(encode(&(
                        charter, payee, amount, purpose, evidence, run.digest,
                    ))),
                    formula: formulas::bonded_statement(charter_state.agent_atom),
                    adjudicator: None,
                    detection_permille: 1000,
                    valid_for: 604_800.min(run.ctx.config.max_claim_window),
                    challenge_period: 86_400,
                };
                let id = claim_id(&tx.sender, &body);
                run.effects.deferred.push(Deferred::IssueClaim {
                    issuer: tx.sender,
                    backer: charter_state.backer,
                    body,
                });
                Some(id)
            } else {
                None
            };
            run.event(Event::AgentPaid {
                charter: *charter,
                payee: *payee,
                amount: *amount,
                receipt,
            });
        }
        TxKind::RevokeCharter { charter } => {
            let mut object = run.load(charter)?;
            let ObjectData::Charter(state) = &mut object.data else {
                return Err(ExecError::Invalid("not a charter".into()));
            };
            if state.principal != tx.sender {
                return Err(ExecError::Unauthorized(
                    "only the principal may revoke".into(),
                ));
            }
            if state.revoked_at.is_none() {
                state.revoked_at = Some(pulse);
            }
            run.store(object);
            run.event(Event::CharterRevoked { charter: *charter });
        }
        TxKind::CreateBounded {
            label,
            asset,
            total,
            allotments,
        } => {
            let allotted = Amount::sum(allotments.iter().map(|(_, a)| a))?;
            if allotted > *total {
                return Err(ExecError::Invalid("allotments exceed the total".into()));
            }
            run.draw(*asset, *total)?;
            let bounded_id = created_id(&run.digest, run.next_index);
            run.create(
                Owner::Shared(object_domain("bounded", &bounded_id)),
                ObjectData::Bounded(BoundedTotal {
                    label: label.clone(),
                    asset: *asset,
                    manager: tx.sender,
                    total: *total,
                    unallocated: total.checked_sub(allotted)?,
                    allotments: BTreeMap::new(),
                    returned: Amount::ZERO,
                }),
            );
            let mut created = Vec::new();
            let mut record = BTreeMap::new();
            for (writer, amount) in allotments {
                let id = run.create(
                    Owner::Account(*writer),
                    ObjectData::Allotment(Allotment {
                        source: bounded_id,
                        writer: *writer,
                        asset: *asset,
                        remaining: *amount,
                        caveats: None,
                        window_start: pulse,
                        spent_in_window: Amount::ZERO,
                    }),
                );
                record.insert(id, (*writer, *amount));
                created.push(id);
            }
            let mut bounded = run.load(&bounded_id)?;
            if let ObjectData::Bounded(b) = &mut bounded.data {
                b.allotments = record;
            }
            run.store(bounded);
            run.event(Event::BoundedCreated {
                bounded: bounded_id,
                allotments: created,
            });
        }
        TxKind::SpendAllotment { to, amount } => {
            let allotment_id = run.input(1)?;
            let mut allotment = load_allotment(run, &allotment_id)?;
            if allotment.caveats.is_some() {
                return Err(ExecError::Invalid(
                    "charter allotments are spent with AgentPay".into(),
                ));
            }
            if allotment.writer != tx.sender {
                return Err(ExecError::NotOwner(allotment_id));
            }
            allotment.spend(to, *amount, pulse, None)?;
            let asset = allotment.asset;
            store_allotment(run, allotment_id, allotment)?;
            run.mint_coin(*to, asset, *amount);
            run.event(Event::AllotmentUsed {
                allotment: allotment_id,
                amount: *amount,
            });
        }
        TxKind::ReturnAllotment => {
            let allotment_id = run.input(1)?;
            let allotment = load_allotment(run, &allotment_id)?;
            if allotment.caveats.is_some() {
                return Err(ExecError::Invalid(
                    "charter allotments cannot be returned to a bounded total".into(),
                ));
            }
            run.consume(&allotment_id)?;
            run.effects.deferred.push(Deferred::ReturnToBounded {
                bounded: allotment.source,
                amount: allotment.remaining,
            });
            run.event(Event::AllotmentUsed {
                allotment: allotment_id,
                amount: allotment.remaining,
            });
        }
        TxKind::Rebalance {
            bounded,
            holder,
            amount,
        } => {
            let mut object = run.load(bounded)?;
            let ObjectData::Bounded(state) = &mut object.data else {
                return Err(ExecError::Invalid("not a bounded total".into()));
            };
            if state.manager != tx.sender {
                return Err(ExecError::Unauthorized(
                    "only the manager may rebalance".into(),
                ));
            }
            state.unallocated = state.unallocated.checked_sub(*amount).map_err(|_| {
                ExecError::InsufficientFunds {
                    need: amount.to_string(),
                    have: state.unallocated.to_string(),
                }
            })?;
            let asset = state.asset;
            let allotment = created_id(&run.digest, run.next_index);
            state.allotments.insert(allotment, (*holder, *amount));
            run.store(object);
            run.create(
                Owner::Account(*holder),
                ObjectData::Allotment(Allotment {
                    source: *bounded,
                    writer: *holder,
                    asset,
                    remaining: *amount,
                    caveats: None,
                    window_start: pulse,
                    spent_in_window: Amount::ZERO,
                }),
            );
            run.event(Event::AllotmentUsed {
                allotment,
                amount: *amount,
            });
        }
        TxKind::CreateMarket {
            label,
            good,
            numeraire,
            batch_length,
            clear_deadline,
            committee,
            threshold,
        } => {
            if *batch_length == 0 || *clear_deadline == 0 {
                return Err(ExecError::Invalid(
                    "batch length and deadline must be positive".into(),
                ));
            }
            if !committee.is_empty()
                && (*threshold == 0 || usize::from(*threshold) > committee.len())
            {
                return Err(ExecError::Invalid(
                    "threshold must be in 1..=committee size".into(),
                ));
            }
            if good == numeraire {
                return Err(ExecError::Invalid("good and numeraire must differ".into()));
            }
            let market_id = created_id(&run.digest, run.next_index);
            run.create(
                Owner::Shared(object_domain("market", &market_id)),
                ObjectData::Market(Box::new(Market {
                    label: label.clone(),
                    domain: object_domain("market", &market_id),
                    good: *good,
                    numeraire: *numeraire,
                    batch_length: *batch_length,
                    clear_deadline: *clear_deadline,
                    committee: committee.clone(),
                    threshold: *threshold,
                    batches: BTreeMap::new(),
                })),
            );
            run.event(Event::MarketCreated { market: market_id });
        }
        TxKind::SubmitOrder { market, submission } => {
            let object = run.load(market)?;
            let ObjectData::Market(state) = &object.data else {
                return Err(ExecError::Invalid("not a market".into()));
            };
            let (escrow_asset, escrow, terms, sealed) = match submission {
                Submission::Plain(terms) => {
                    if terms.quantity == 0 {
                        return Err(ExecError::Invalid("zero quantity".into()));
                    }
                    let (asset, amount) = terms.escrow(state);
                    (asset, amount, Some(*terms), None)
                }
                Submission::Sealed {
                    envelope,
                    side,
                    escrow,
                } => {
                    if state.committee.is_empty() {
                        return Err(ExecError::Invalid(
                            "market does not accept sealed orders".into(),
                        ));
                    }
                    // Sealed to this market's committee at its threshold, so
                    // the owner cannot reveal (or withhold) the order alone.
                    let members: Vec<Hash32> =
                        state.committee.iter().map(|k| k.fingerprint()).collect();
                    if envelope.members != members || envelope.threshold != state.threshold {
                        return Err(ExecError::Invalid(
                            "sealed orders must be sealed to the market's committee at its threshold".into(),
                        ));
                    }
                    if *escrow == Amount::ZERO {
                        return Err(ExecError::Invalid("zero escrow".into()));
                    }
                    let asset = match side {
                        Side::Buy => state.numeraire,
                        Side::Sell => state.good,
                    };
                    (asset, *escrow, None, Some(envelope.clone()))
                }
            };
            run.draw(escrow_asset, escrow)?;
            let order_id = hash_parts("fold/market/order", &[&run.digest.0 .0]);
            run.effects.deferred.push(Deferred::AddOrder {
                market: *market,
                order: PendingOrder {
                    id: order_id,
                    owner: tx.sender,
                    escrow_asset,
                    escrow,
                    terms,
                    sealed,
                },
            });
            run.event(Event::OrderSubmitted {
                market: *market,
                order: order_id,
            });
        }
        TxKind::RevealBatch {
            market,
            batch,
            shares,
        } => {
            let mut object = run.load(market)?;
            let ObjectData::Market(state) = &mut object.data else {
                return Err(ExecError::Invalid("not a market".into()));
            };
            let domain = state.domain;
            let numeraire = state.numeraire;
            let good = state.good;
            let entry = state
                .batches
                .get_mut(batch)
                .ok_or_else(|| ExecError::Invalid("unknown batch".into()))?;
            if !matches!(entry.status, BatchStatus::Sealed { .. }) {
                return Err(ExecError::Invalid("batch is not sealed".into()));
            }
            let mut revealed = 0u64;
            let provided: BTreeMap<Hash32, &Vec<fold_crypto::KeyShare>> =
                shares.iter().map(|(id, s)| (*id, s)).collect();
            for order in &mut entry.orders {
                let (Some(envelope), None) = (&order.sealed, &order.terms) else {
                    continue;
                };
                let Some(order_shares) = provided.get(&order.id) else {
                    continue;
                };
                let Ok(bytes) = envelope.open(
                    order_shares,
                    SEALED_ORDER_DOMAIN,
                    &sealing_aad(&domain, *batch),
                ) else {
                    continue;
                };
                let Ok(terms) = decode::<OrderTerms>(&bytes) else {
                    continue;
                };
                let (asset, need) = match terms.side {
                    Side::Buy => (
                        numeraire,
                        Amount(
                            terms
                                .limit_price
                                .0
                                .saturating_mul(u128::from(terms.quantity)),
                        ),
                    ),
                    Side::Sell => (good, Amount::units(terms.quantity)),
                };
                if asset == order.escrow_asset && need <= order.escrow && terms.quantity > 0 {
                    order.terms = Some(terms);
                    revealed += 1;
                }
            }
            run.store(object);
            run.event(Event::BatchRevealed {
                market: *market,
                batch: *batch,
                revealed,
            });
        }
        TxKind::ClearBatch {
            market,
            batch,
            result,
        } => {
            let mut object = run.load(market)?;
            let ObjectData::Market(state) = &mut object.data else {
                return Err(ExecError::Invalid("not a market".into()));
            };
            let entry = state
                .batches
                .get(batch)
                .cloned()
                .ok_or_else(|| ExecError::Invalid("unknown batch".into()))?;
            let BatchStatus::Sealed { beacon, at } = entry.status else {
                return Err(ExecError::Invalid("batch is not awaiting clearing".into()));
            };
            // Sealed orders get the reveal window: while any is unrevealed, a
            // clearing may only land in the last Pulse before the deadline,
            // so a solver cannot exclude orders by clearing early.
            let unrevealed = entry.orders.iter().any(|o| o.terms.is_none());
            if unrevealed && pulse.0 < at.0 + state.clear_deadline.saturating_sub(1) {
                return Err(ExecError::Invalid(
                    "sealed orders are still being revealed".into(),
                ));
            }
            let clearing = state.clearing_batch(&entry, beacon, at);
            let verified = verify_clearing(&clearing, result)?;
            let price = verified.price;
            let fills: BTreeMap<Hash32, u64> = result
                .fills
                .iter()
                .map(|f| (f.order_id, f.quantity))
                .collect();
            let (good, numeraire) = (state.good, state.numeraire);
            for order in &entry.orders {
                let filled = fills.get(&order.id).copied().unwrap_or(0);
                let (receive_asset, receive, refund) = match (order.terms, price) {
                    (Some(terms), Some(price)) if filled > 0 => {
                        let value = Amount(price.0 * u128::from(filled));
                        match terms.side {
                            Side::Buy => (
                                good,
                                Amount::units(filled),
                                order.escrow.checked_sub(value)?,
                            ),
                            Side::Sell => (
                                numeraire,
                                value,
                                order.escrow.checked_sub(Amount::units(filled))?,
                            ),
                        }
                    }
                    _ => (order.escrow_asset, Amount::ZERO, order.escrow),
                };
                run.mint_coin(order.owner, receive_asset, receive);
                run.mint_coin(order.owner, order.escrow_asset, refund);
            }
            if let Some(entry) = state.batches.get_mut(batch) {
                entry.status = BatchStatus::Cleared {
                    price,
                    volume: verified.volume,
                    welfare: verified.welfare,
                    solver: tx.sender,
                };
            }
            run.store(object);
            run.event(Event::BatchCleared {
                market: *market,
                batch: *batch,
                price,
                volume: verified.volume,
                welfare: verified.welfare,
            });
        }
    }
    Ok(())
}

/// The atom `registrant` registers as `label`.
pub fn registered_atom(class: AtomClass, registrant: &AccountId, label: &str) -> Atom {
    Atom::new(class, format!("registered:{registrant}:{label}"))
}

/// The bonded atom for `owner`'s bond named `label`. Bonded atoms carry
/// their owner in the label, and only the owner can post the first bond
/// behind one (see [`bond_owner`]).
pub fn bond_atom(class: AtomClass, owner: &AccountId, label: &str) -> Atom {
    Atom::bonded(class, format!("bond:{owner}:{label}"))
}

/// The account a bonded atom belongs to, read from its label.
pub fn bond_owner(atom: &Atom) -> Option<AccountId> {
    if !atom.bonded {
        return None;
    }
    let rest = atom.label.strip_prefix("bond:")?;
    let (owner, _) = rest.split_once(':')?;
    fold_core::hash::Hash32::from_hex(owner)
        .ok()
        .map(AccountId::from_hash)
}

fn validate_claim_body(config: &ChainConfig, body: &ClaimBody) -> Result<(), ExecError> {
    if body.schema.is_empty() || body.schema.len() > 128 {
        return Err(ExecError::Invalid("schema must be 1 to 128 bytes".into()));
    }
    if body.subject.len() > 512 {
        return Err(ExecError::Invalid("subject too long".into()));
    }
    if body.payload.size() > config.max_payload_bytes {
        return Err(ExecError::Invalid("payload too large".into()));
    }
    if body.formula.size() > config.max_formula_size {
        return Err(ExecError::Invalid("formula too large".into()));
    }
    if body.detection_permille > 1000 {
        return Err(ExecError::Invalid("detection factor above one".into()));
    }
    if body.valid_for == 0 || body.valid_for > config.max_claim_window {
        return Err(ExecError::Invalid("validity window out of range".into()));
    }
    if body.formula == Formula::False {
        return Err(ExecError::Invalid("a claim cannot rest on ⊥".into()));
    }
    Ok(())
}

fn load_allotment<S: ObjectStore>(
    run: &mut Run<'_, '_, S>,
    id: &ObjectId,
) -> Result<Allotment, ExecError> {
    let object = run.load(id)?;
    match object.data {
        ObjectData::Allotment(allotment) => Ok(allotment),
        _ => Err(ExecError::Invalid("input 1 is not an allotment".into())),
    }
}

fn store_allotment<S: ObjectStore>(
    run: &mut Run<'_, '_, S>,
    id: ObjectId,
    allotment: Allotment,
) -> Result<(), ExecError> {
    let mut object = run.load(&id)?;
    object.data = ObjectData::Allotment(allotment);
    run.store(object);
    Ok(())
}

fn load_charter<S: ObjectStore>(
    run: &mut Run<'_, '_, S>,
    id: &ObjectId,
) -> Result<Charter, ExecError> {
    let object = run.load(id)?;
    match object.data {
        ObjectData::Charter(charter) => Ok(charter),
        _ => Err(ExecError::Invalid("not a charter".into())),
    }
}

/// A charter and all its ancestors must be unrevoked.
fn check_charter_chain<S: ObjectStore>(
    run: &mut Run<'_, '_, S>,
    id: &ObjectId,
) -> Result<(), ExecError> {
    let mut current = Some(*id);
    let mut depth = 0;
    while let Some(charter_id) = current {
        let charter = load_charter(run, &charter_id)?;
        if charter.revoked_at.is_some() {
            return Err(ExecError::Caveat("charter revoked".into()));
        }
        current = charter.parent;
        depth += 1;
        if depth > 16 {
            return Err(ExecError::Invalid("delegation chain too deep".into()));
        }
    }
    Ok(())
}

fn evidence_passes<S: ObjectStore>(
    run: &mut Run<'_, '_, S>,
    caveats: &Caveats,
    claim: &ClaimId,
    amount: Amount,
) -> Result<bool, ExecError> {
    let (_, domain) = run.assurance()?;
    let Some(record) = domain.registry.claim(claim) else {
        return Ok(false);
    };
    if record.status != ClaimStatus::Active {
        return Ok(false);
    }
    let decision = decide(
        &domain.registry,
        &caveats.evidence_policy,
        claim,
        amount,
        false,
    )?;
    Ok(decision.accept)
}

/// Turns a slashing outcome into payments: compensation to consumers, a
/// share of released pools to the challenger, the rest to the treasury.
fn settle_slash<S: ObjectStore>(
    run: &mut Run<'_, '_, S>,
    domain: &mut AssuranceDomain,
    outcome: SlashOutcome,
    challenger: Option<AccountId>,
) -> Result<(), ExecError> {
    run.event(Event::Slashed {
        atom: outcome.atom,
        amount: outcome.slashed,
    });
    for falsified in &outcome.falsified {
        run.event(Event::Falsified {
            claim: falsified.claim,
            causes: falsified.causes.clone(),
        });
    }
    for compensation in &outcome.compensations {
        domain.vault = domain.vault.checked_sub(compensation.amount)?;
        run.mint_coin(compensation.consumer, AssetId::usd(), compensation.amount);
        run.event(Event::Compensated {
            reservation: compensation.reservation,
            consumer: compensation.consumer,
            amount: compensation.amount,
        });
    }
    let released = Amount::sum(outcome.released.values())?;
    domain.vault = domain.vault.checked_sub(released)?;
    let share = u128::from(run.ctx.config.challenger_share_permille);
    let to_challenger = match challenger {
        Some(_) => released.mul_ratio(share, 1000)?,
        None => Amount::ZERO,
    };
    if let Some(challenger) = challenger {
        run.mint_coin(challenger, AssetId::usd(), to_challenger);
    }
    let to_treasury = released.checked_sub(to_challenger)?;
    if to_treasury > Amount::ZERO {
        run.effects.deferred.push(Deferred::Treasury {
            asset: AssetId::usd(),
            amount: to_treasury,
        });
    }
    Ok(())
}
