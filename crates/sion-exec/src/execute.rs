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
use serde::{Deserialize, Serialize};
use sion_assurance::{
    accept_and_reserve, decide, release, Atom, AtomClass, ClaimStatus, Formula, SlashOutcome,
};
use sion_clearing::{verify_clearing, Side};
use sion_core::encode::{decode, encode};
use sion_core::hash::{hash_parts, Hash32};
use sion_core::ids::{
    AccountId, AssetId, AtomId, ClaimId, ObjectId, Pulse, TxDigest, ValidatorId, Version,
};
use sion_core::{merkle, Amount};

use crate::account::AccountState;
use crate::assurance::AssuranceDomain;
use crate::charter::{
    AgentIdentity, Allotment, Caveats, Charter, Spawn, MAX_CHARTER_CHAIN, MAX_SPAWN_BATCH,
};
use crate::claim::{claim_id, formulas, ClaimBody, Payload};
use crate::error::ExecError;
use crate::escrow::BoundedTotal;
use crate::market::{
    sealing_aad, BatchStatus, Market, OrderTerms, PendingOrder, Submission, SEALED_ORDER_DOMAIN,
};
use crate::memory::{MemoryLog, MAX_CONSISTENCY_PROOF};
use crate::object::{
    created_id, well_known, Coin, CoordinationClass, Object, ObjectData, Owner, ScheduledEvent,
    Tombstone,
};
use crate::state::ChainConfig;
use crate::store::{ObjectStore, Overlay};
use crate::tx::{
    object_domain, signing_context, SignedTransaction, Transaction, TxKind, MAX_GRANT, MAX_INPUTS,
};
use crate::validators::{
    permille_of, possession_digest, ValidatorRecord, ValidatorSet, ValidatorStatus,
};

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
    /// Credit a transaction fee to the treasury: its balance and its running
    /// total of fees, from which validators are paid.
    Fee {
        /// Asset.
        asset: AssetId,
        /// Amount.
        amount: Amount,
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
    /// A dead charter's allotments were returned to its root principal.
    CharterReclaimed {
        /// Charter.
        charter: ObjectId,
        /// Amount returned.
        amount: Amount,
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
    /// A validator staked; it joins the active set at the next epoch.
    ValidatorRegistered {
        /// The validator.
        validator: ValidatorId,
        /// Its operator.
        operator: AccountId,
        /// Stake.
        stake: Amount,
    },
    /// A validator leaves the active set at the next epoch boundary (or
    /// left at once, if it had not joined yet).
    ValidatorLeaving {
        /// The validator.
        validator: ValidatorId,
    },
    /// Stake (and any unclaimed rewards) were withdrawn.
    StakeWithdrawn {
        /// The validator.
        validator: ValidatorId,
        /// Stake returned to the operator (in the stake asset).
        amount: Amount,
    },
    /// Rewards were claimed.
    RewardsClaimed {
        /// The validator.
        validator: ValidatorId,
        /// Amount paid to the operator.
        amount: Amount,
    },
    /// A validator was slashed for equivocation: its stake and unclaimed
    /// rewards are forfeited, the reporter receives its share of each and
    /// the treasury the rest.
    ValidatorSlashed {
        /// The offender.
        validator: ValidatorId,
        /// Stake forfeited (in the stake asset).
        amount: Amount,
        /// Who reported it.
        reporter: AccountId,
        /// The reporter's share of the stake.
        reward: Amount,
    },
    /// An epoch began: rewards paid, validators admitted and retired,
    /// committees sampled afresh.
    EpochChanged {
        /// The new epoch.
        epoch: u64,
        /// Validators in its active set.
        active: u32,
        /// Rewards paid for the epoch that ended.
        rewards: Amount,
    },
    /// A forced intent became an obligation of its sender's shard.
    ForcedPosted {
        /// The intent.
        intent: TxDigest,
        /// The shard.
        shard: u16,
        /// The Pulse at whose end it is checked.
        deadline: Pulse,
    },
    /// A posted intent created no obligation.
    ForcedDismissed {
        /// The intent.
        intent: TxDigest,
        /// Why (free text, not committed).
        reason: String,
    },
    /// An obligation was met: the intent can no longer execute.
    ForcedResolved {
        /// The intent.
        intent: TxDigest,
    },
    /// A shard missed a forced-inclusion deadline: the intent could still
    /// execute and its shard did not execute it.
    ForcedOverdue {
        /// The intent.
        intent: TxDigest,
        /// The shard.
        shard: u16,
    },
    /// A shard's committee was frozen out and replaced.
    ShardRotated {
        /// The shard.
        shard: u16,
        /// Rotations of this shard in the epoch so far.
        rotation: u32,
    },
    /// A payment stream was opened.
    StreamOpened {
        /// The stream.
        stream: ObjectId,
        /// Who escrowed.
        payer: AccountId,
        /// Who is paid.
        payee: AccountId,
        /// The escrow.
        deposit: Amount,
    },
    /// A payee settled steps of a stream.
    StreamSettled {
        /// The stream.
        stream: ObjectId,
        /// The position settled to.
        position: u64,
        /// The chain element at that position.
        head: Hash32,
        /// What the payee received.
        amount: Amount,
    },
    /// A payer added to a stream's escrow or extended it.
    StreamToppedUp {
        /// The stream.
        stream: ObjectId,
        /// Added to the escrow.
        deposit: Amount,
        /// The stream's last Pulse.
        until: Pulse,
    },
    /// A stream was closed and the rest of its escrow returned.
    StreamClosed {
        /// The stream.
        stream: ObjectId,
        /// Returned to the payer.
        refund: Amount,
    },
    /// An agent identity was registered (spec §5.35).
    AgentRegistered {
        /// The agent's id (its charter).
        agent: ObjectId,
        /// The account that signs for it.
        account: AccountId,
        /// Its allotment.
        allotment: ObjectId,
    },
    /// A subagent was launched (spec §5.36).
    AgentSpawned {
        /// The subagent's id (its charter).
        agent: ObjectId,
        /// Its parent's charter.
        parent: ObjectId,
        /// The account that signs for it.
        account: AccountId,
        /// Its allotment.
        allotment: ObjectId,
    },
    /// An agent replaced its card commitment (spec §5.37).
    AgentCardSet {
        /// The agent.
        agent: ObjectId,
        /// The new commitment.
        card: Hash32,
    },
    /// A charter and its subtree were suspended (spec §5.38).
    AgentSuspended {
        /// The charter.
        agent: ObjectId,
        /// Who holds the suspension.
        by: AccountId,
    },
    /// A suspension was lifted (spec §5.39).
    AgentResumed {
        /// The charter.
        agent: ObjectId,
    },
    /// A memory log was opened (spec §5.40).
    MemoryOpened {
        /// The log.
        log: ObjectId,
        /// The account that writes it. (Not named `writer`: Borsh's derive
        /// uses that name for its output.)
        account: AccountId,
        /// The agent identity it belongs to, if any.
        agent: Option<ObjectId>,
    },
    /// A memory log's head moved forward (spec §5.41). The atoms it added
    /// have this Pulse as their transaction time.
    MemoryCommitted {
        /// The log.
        log: ObjectId,
        /// Its size after the commit.
        size: u64,
        /// Its root after the commit.
        root: Hash32,
        /// The head's sequence number.
        commits: u64,
        /// Whether the log is now sealed.
        sealed: bool,
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
            self.effects.deferred.push(Deferred::Fee {
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
        self.effects.deferred.push(Deferred::Fee {
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

    fn validators(&mut self) -> Result<(Object, ValidatorSet), ExecError> {
        let object = self.load(&well_known::validators())?;
        let ObjectData::Validators(set) = &object.data else {
            return Err(ExecError::Invalid("validator set corrupted".into()));
        };
        let set = (**set).clone();
        Ok((object, set))
    }

    fn store_validators(&mut self, mut object: Object, set: ValidatorSet) {
        object.data = ObjectData::Validators(Box::new(set));
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
    if tx.inputs.is_empty() || tx.inputs.len() > MAX_INPUTS {
        return Err(ExecError::Invalid(format!(
            "a transaction needs 1 to {MAX_INPUTS} owned inputs"
        )));
    }
    let unique: BTreeSet<ObjectId> = tx.inputs.iter().map(|r| r.id).collect();
    if unique.len() != tx.inputs.len() {
        return Err(ExecError::Invalid("duplicate inputs".into()));
    }
    let is_faucet = matches!(tx.kind, TxKind::Faucet { .. });
    if is_faucet {
        if tx.fee != Amount::ZERO {
            return Err(ExecError::Invalid("faucet transactions pay no fee".into()));
        }
    } else {
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
            .verify(
                &signing_context(&ctx.config),
                &run.digest.0 .0,
                &signed.auth,
            )
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
                if *amount > MAX_GRANT {
                    return Err(ExecError::Invalid(format!(
                        "a grant may be at most {MAX_GRANT}"
                    )));
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
            let (charter, allotment) =
                create_root_charter(run, agent, caveats, *funding, *agent_bond, None)?;
            run.event(Event::CharterCreated { charter, allotment });
        }
        TxKind::RegisterAgent {
            agent,
            card,
            caveats,
            funding,
            agent_bond,
        } => {
            let identity = AgentIdentity {
                card: *card,
                depth: 0,
            };
            let (charter, allotment) =
                create_root_charter(run, agent, caveats, *funding, *agent_bond, Some(identity))?;
            run.event(Event::AgentRegistered {
                agent: charter,
                account: *agent,
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
            carve_window(&mut allotment, &parent_caveats, &caveats.max_per_window)?;
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
                    suspended_by: None,
                    identity: None,
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
                    carved_window: Amount::ZERO,
                }),
            );
            let mut child = run.load(&child_id)?;
            if let ObjectData::Charter(c) = &mut child.data {
                c.allotments.insert(child_allotment);
            }
            run.store(child);
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
            let state = load_charter(run, charter)?;
            if state.agent != tx.sender && !is_overseer(run, &state, &tx.sender)? {
                return Err(ExecError::Unauthorized(
                    "only the agent or an overseer (its backer, principal or an ancestor's agent) may revoke".into(),
                ));
            }
            let mut object = run.load(charter)?;
            if let ObjectData::Charter(state) = &mut object.data {
                if state.revoked_at.is_none() {
                    state.revoked_at = Some(pulse);
                }
            }
            run.store(object);
            run.event(Event::CharterRevoked { charter: *charter });
        }
        TxKind::ReclaimCharter { charter } => {
            let state = load_charter(run, charter)?;
            if tx.sender != state.principal && tx.sender != state.backer {
                return Err(ExecError::Unauthorized(
                    "only the principal or the root principal may reclaim".into(),
                ));
            }
            let dead = state.revoked_at.is_some()
                || pulse > state.caveats.expires_at
                || charter_chain(run, charter, false).is_err();
            if !dead {
                return Err(ExecError::Invalid(
                    "the charter is still live; revoke it first".into(),
                ));
            }
            let mut total = Amount::ZERO;
            for allotment_id in &state.allotments {
                let object = run.load(allotment_id)?;
                if let ObjectData::Allotment(allotment) = &object.data {
                    total = total.checked_add(allotment.remaining)?;
                    run.consume(allotment_id)?;
                }
            }
            run.mint_coin(state.backer, AssetId::usd(), total);
            run.event(Event::CharterReclaimed {
                charter: *charter,
                amount: total,
            });
        }
        TxKind::SpawnAgents { charter, children } => {
            if children.is_empty() || children.len() > MAX_SPAWN_BATCH {
                return Err(ExecError::Invalid(format!(
                    "between 1 and {MAX_SPAWN_BATCH} subagents per transaction"
                )));
            }
            let allotment_id = run.input(1)?;
            let mut allotment = load_allotment(run, &allotment_id)?;
            let parent = load_charter(run, charter)?;
            let depth = check_charter_chain(run, charter)?;
            if depth >= MAX_CHARTER_CHAIN {
                return Err(ExecError::Invalid("delegation chain too deep".into()));
            }
            if allotment.source != *charter
                || allotment.writer != tx.sender
                || parent.agent != tx.sender
            {
                return Err(ExecError::Invalid(
                    "allotment does not belong to this charter's agent".into(),
                ));
            }
            let own = allotment
                .caveats
                .clone()
                .ok_or_else(|| ExecError::Invalid("not a charter allotment".into()))?;
            if !own.may_delegate {
                return Err(ExecError::Caveat("delegation not allowed".into()));
            }
            for child in children {
                if !child.caveats.attenuates(&own) {
                    return Err(ExecError::Caveat(
                        "a subagent's caveats must be at least as strict as its parent's".into(),
                    ));
                }
                carve_window(&mut allotment, &own, &child.caveats.max_per_window)?;
                allotment.remaining =
                    allotment.remaining.checked_sub(child.budget).map_err(|_| {
                        ExecError::InsufficientFunds {
                            need: child.budget.to_string(),
                            have: allotment.remaining.to_string(),
                        }
                    })?;
            }
            store_allotment(run, allotment_id, allotment)?;
            for child in children {
                spawn_one(run, charter, &parent, child, depth)?;
            }
        }
        TxKind::SetAgentCard { charter, card } => {
            let state = load_charter(run, charter)?;
            if state.identity.is_none() {
                return Err(ExecError::Invalid("not an agent identity".into()));
            }
            if state.agent != tx.sender {
                return Err(ExecError::Unauthorized(
                    "only the agent may set its card".into(),
                ));
            }
            check_charter_chain(run, charter)?;
            let mut object = run.load(charter)?;
            if let ObjectData::Charter(c) = &mut object.data {
                if let Some(identity) = &mut c.identity {
                    identity.card = *card;
                }
            }
            run.store(object);
            run.event(Event::AgentCardSet {
                agent: *charter,
                card: *card,
            });
        }
        TxKind::SuspendAgent { charter } => {
            let state = load_charter(run, charter)?;
            if !is_overseer(run, &state, &tx.sender)? {
                return Err(ExecError::Unauthorized(
                    "only an overseer (its backer, principal or an ancestor's agent) may suspend"
                        .into(),
                ));
            }
            let mut object = run.load(charter)?;
            let mut by = tx.sender;
            if let ObjectData::Charter(c) = &mut object.data {
                by = *c.suspended_by.get_or_insert(tx.sender);
            }
            run.store(object);
            run.event(Event::AgentSuspended {
                agent: *charter,
                by,
            });
        }
        TxKind::ResumeAgent { charter } => {
            let state = load_charter(run, charter)?;
            let Some(by) = state.suspended_by else {
                return Err(ExecError::Invalid("the charter is not suspended".into()));
            };
            if tx.sender != by && tx.sender != state.backer {
                return Err(ExecError::Unauthorized(
                    "only the suspender or the backer may resume".into(),
                ));
            }
            let mut object = run.load(charter)?;
            if let ObjectData::Charter(c) = &mut object.data {
                c.suspended_by = None;
            }
            run.store(object);
            run.event(Event::AgentResumed { agent: *charter });
        }
        TxKind::CreateBounded {
            label,
            asset,
            total,
            allotments,
        } => {
            if allotments.len() > crate::escrow::MAX_ALLOTMENTS {
                return Err(ExecError::Invalid(format!(
                    "at most {} allotments",
                    crate::escrow::MAX_ALLOTMENTS
                )));
            }
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
                        carved_window: Amount::ZERO,
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
                    carved_window: Amount::ZERO,
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
            let period = 1..=crate::market::MAX_PERIOD;
            if !period.contains(batch_length) || !period.contains(clear_deadline) {
                return Err(ExecError::Invalid(format!(
                    "batch length and deadline must be between 1 and {} Pulses",
                    crate::market::MAX_PERIOD
                )));
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
            let order_id = hash_parts("sion/market/order", &[&run.digest.0 .0]);
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
            let provided: BTreeMap<Hash32, &Vec<sion_crypto::KeyShare>> =
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
            if unrevealed && pulse.0 < at.0.saturating_add(state.clear_deadline.saturating_sub(1)) {
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
                        let value =
                            Amount(price.0.checked_mul(u128::from(filled)).ok_or_else(|| {
                                ExecError::Invalid("clearing value overflows".into())
                            })?);
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
        TxKind::RegisterValidator {
            lattice,
            hash,
            beacon_tip,
            stake,
            possession,
        } => {
            let id = sion_crypto::validator_id(lattice, hash);
            let (object, mut set) = run.validators()?;
            if set.validators.contains_key(&id) {
                return Err(ExecError::Invalid("validator already registered".into()));
            }
            if *stake < set.params.min_stake {
                return Err(ExecError::Invalid(format!(
                    "stake {stake} below the minimum {}",
                    set.params.min_stake
                )));
            }
            let message = possession_digest(&run.ctx.config, &tx.sender, &id);
            if !sion_crypto::verify_validator(lattice, hash, &message.0, possession) {
                return Err(ExecError::Unauthorized(
                    "no proof of possession of the validator keys".into(),
                ));
            }
            run.draw(set.params.stake_asset, *stake)?;
            set.validators.insert(
                id,
                ValidatorRecord {
                    id,
                    lattice: lattice.clone(),
                    hash: hash.clone(),
                    operator: tx.sender,
                    stake: *stake,
                    status: ValidatorStatus::Joining,
                    beacon_tip: *beacon_tip,
                    rewards: Amount::ZERO,
                    registered_at: pulse,
                },
            );
            run.store_validators(object, set);
            run.event(Event::ValidatorRegistered {
                validator: id,
                operator: tx.sender,
                stake: *stake,
            });
        }
        TxKind::Unstake { validator } => {
            let (object, mut set) = run.validators()?;
            let staying = set
                .validators
                .values()
                .filter(|r| {
                    r.id != *validator
                        && matches!(r.status, ValidatorStatus::Active | ValidatorStatus::Joining)
                })
                .count();
            let record = operated(&mut set, validator, &tx.sender)?;
            match record.status {
                ValidatorStatus::Joining => {
                    record.status = ValidatorStatus::Exited {
                        withdrawable_at: pulse,
                    }
                }
                ValidatorStatus::Active => {
                    if staying == 0 {
                        return Err(ExecError::Invalid("the last validator cannot leave".into()));
                    }
                    record.status = ValidatorStatus::Leaving;
                }
                _ => {
                    return Err(ExecError::Invalid(
                        "the validator is neither active nor joining".into(),
                    ))
                }
            }
            run.store_validators(object, set);
            run.event(Event::ValidatorLeaving {
                validator: *validator,
            });
        }
        TxKind::WithdrawStake { validator } => {
            let (object, mut set) = run.validators()?;
            let record = operated(&mut set, validator, &tx.sender)?;
            match record.status {
                ValidatorStatus::Exited { withdrawable_at } if withdrawable_at <= pulse => {}
                ValidatorStatus::Exited { withdrawable_at } => {
                    return Err(ExecError::Invalid(format!(
                        "stake is withdrawable from Pulse {withdrawable_at}"
                    )))
                }
                _ => {
                    return Err(ExecError::Invalid(
                        "only an exited validator's stake can be withdrawn".into(),
                    ))
                }
            }
            let (stake, rewards) = (record.stake, record.rewards);
            record.stake = Amount::ZERO;
            record.rewards = Amount::ZERO;
            record.status = ValidatorStatus::Withdrawn;
            let asset = set.params.stake_asset;
            run.store_validators(object, set);
            run.mint_coin(tx.sender, asset, stake);
            run.mint_coin(tx.sender, AssetId::usd(), rewards);
            run.event(Event::StakeWithdrawn {
                validator: *validator,
                amount: stake,
            });
        }
        TxKind::ClaimRewards { validator } => {
            let (object, mut set) = run.validators()?;
            let record = operated(&mut set, validator, &tx.sender)?;
            let amount = record.rewards;
            if amount == Amount::ZERO {
                return Err(ExecError::Invalid("no rewards to claim".into()));
            }
            record.rewards = Amount::ZERO;
            run.store_validators(object, set);
            run.mint_coin(tx.sender, AssetId::usd(), amount);
            run.event(Event::RewardsClaimed {
                validator: *validator,
                amount,
            });
        }
        TxKind::ReportEquivocation { evidence } => {
            let (object, mut set) = run.validators()?;
            let offender = evidence.offender(&set, pulse).map_err(ExecError::Invalid)?;
            let share = set.params.reporter_permille;
            let asset = set.params.stake_asset;
            let record = set
                .validators
                .get_mut(&offender)
                .ok_or_else(|| ExecError::Invalid("unknown validator".into()))?;
            if matches!(
                record.status,
                ValidatorStatus::Slashed { .. } | ValidatorStatus::Withdrawn
            ) {
                return Err(ExecError::Invalid(
                    "the validator was already slashed or has withdrawn".into(),
                ));
            }
            // Stake and unclaimed rewards are forfeited; the reporter gets
            // its share of each, the treasury the rest.
            let (stake, rewards) = (record.stake, record.rewards);
            record.stake = Amount::ZERO;
            record.rewards = Amount::ZERO;
            record.status = ValidatorStatus::Slashed { at: pulse };
            run.store_validators(object, set);
            let mut paid = [Amount::ZERO; 2];
            for (index, (asset, forfeited)) in [(asset, stake), (AssetId::usd(), rewards)]
                .into_iter()
                .enumerate()
            {
                paid[index] = permille_of(forfeited, share);
                run.mint_coin(tx.sender, asset, paid[index]);
                let rest = forfeited.checked_sub(paid[index])?;
                if rest > Amount::ZERO {
                    run.effects.deferred.push(Deferred::Treasury {
                        asset,
                        amount: rest,
                    });
                }
            }
            run.event(Event::ValidatorSlashed {
                validator: offender,
                amount: stake,
                reporter: tx.sender,
                reward: paid[0],
            });
        }
        TxKind::OpenStream {
            payee,
            asset,
            unit,
            length,
            anchor,
            deposit,
            until,
            checkpoints,
        } => {
            if *unit == Amount::ZERO || *deposit == Amount::ZERO {
                return Err(ExecError::Invalid(
                    "a stream needs a positive unit and deposit".into(),
                ));
            }
            if *length == 0 || *length > crate::stream::MAX_STREAM_LENGTH {
                return Err(ExecError::Invalid(format!(
                    "a stream's length must be between 1 and {}",
                    crate::stream::MAX_STREAM_LENGTH
                )));
            }
            if let Some(c) = checkpoints {
                if c.spacing == 0
                    || c.spacing > *length
                    || length / c.spacing >= crate::stream::MAX_CHECKPOINTS
                {
                    return Err(ExecError::Invalid(format!(
                        "checkpoints need a spacing between 1 and the length, and at most {} of them",
                        crate::stream::MAX_CHECKPOINTS
                    )));
                }
            }
            let latest = pulse.0.saturating_add(crate::market::MAX_PERIOD);
            if until.0 <= pulse.0 || until.0 > latest {
                return Err(ExecError::Invalid(format!(
                    "a stream must last between 1 and {} Pulses",
                    crate::market::MAX_PERIOD
                )));
            }
            run.draw(*asset, *deposit)?;
            let stream = created_id(&run.digest, run.next_index);
            run.create(
                Owner::Shared(object_domain("stream", &stream)),
                ObjectData::Stream(crate::stream::Stream {
                    payer: tx.sender,
                    payee: *payee,
                    asset: *asset,
                    unit: *unit,
                    length: *length,
                    position: 0,
                    head: *anchor,
                    escrow: *deposit,
                    until: *until,
                    open: true,
                    spacing: checkpoints.map(|c| c.spacing).unwrap_or(0),
                    checkpoints: checkpoints
                        .map(|c| c.root)
                        .unwrap_or(sion_core::hash::Hash32::ZERO),
                }),
            );
            run.event(Event::StreamOpened {
                stream,
                payer: tx.sender,
                payee: *payee,
                deposit: *deposit,
            });
        }
        TxKind::SettleStream {
            stream,
            position,
            value,
            jump,
        } => {
            let mut object = run.load(stream)?;
            let ObjectData::Stream(state) = &mut object.data else {
                return Err(ExecError::Invalid(format!("{stream} is not a stream")));
            };
            if !state.open {
                return Err(ExecError::Invalid("the stream is closed".into()));
            }
            if state.payee != tx.sender {
                return Err(ExecError::Unauthorized(
                    "only the stream's payee may settle it".into(),
                ));
            }
            if pulse > state.until {
                return Err(ExecError::Invalid("the stream has expired".into()));
            }
            if *position <= state.position || *position > state.length {
                return Err(ExecError::Invalid(format!(
                    "position {position} is not between {} and {}",
                    state.position + 1,
                    state.length
                )));
            }
            let steps = position - state.position;
            // The hashes this settlement costs the validators.
            let hashes = match jump {
                None if steps > crate::stream::MAX_SETTLE_STEPS => {
                    return Err(ExecError::Invalid(format!(
                        "at most {} steps per settlement without a jump",
                        crate::stream::MAX_SETTLE_STEPS
                    )));
                }
                None => steps,
                Some(jump) => crate::stream::jump_hashes(state, *position, jump).ok_or_else(
                    || {
                        ExecError::Invalid(
                            "the jump does not apply: no checkpoints, a checkpoint not below the position, or a path too long"
                                .into(),
                        )
                    },
                )?,
            };
            let required = run.ctx.config.min_fee(tx.size()).0.saturating_add(
                crate::stream::STREAM_STEP_FEE
                    .0
                    .saturating_mul(u128::from(hashes)),
            );
            if tx.fee.0 < required {
                return Err(ExecError::Invalid(format!(
                    "settling with {hashes} hashes needs a fee of at least {}",
                    Amount(required)
                )));
            }
            let genuine = match jump {
                None => crate::stream::walk(value, steps) == state.head,
                Some(jump) => crate::stream::verify_jump(state, *position, value, jump),
            };
            if !genuine {
                return Err(ExecError::Invalid(
                    "the value is not the chain element at that position".into(),
                ));
            }
            let amount = state
                .unit
                .0
                .checked_mul(u128::from(steps))
                .map(Amount)
                .ok_or_else(|| ExecError::Invalid("the amount overflows".into()))?;
            if amount > state.escrow {
                return Err(ExecError::Invalid(format!(
                    "{amount} is more than the {} left in escrow",
                    state.escrow
                )));
            }
            state.escrow = state.escrow.checked_sub(amount)?;
            state.position = *position;
            state.head = *value;
            let asset = state.asset;
            run.store(object);
            run.mint_coin(tx.sender, asset, amount);
            run.event(Event::StreamSettled {
                stream: *stream,
                position: *position,
                head: *value,
                amount,
            });
        }
        TxKind::TopUpStream {
            stream,
            deposit,
            until,
        } => {
            let mut object = run.load(stream)?;
            let ObjectData::Stream(state) = &mut object.data else {
                return Err(ExecError::Invalid(format!("{stream} is not a stream")));
            };
            if !state.open {
                return Err(ExecError::Invalid("the stream is closed".into()));
            }
            if state.payer != tx.sender {
                return Err(ExecError::Unauthorized(
                    "only the stream's payer may top it up".into(),
                ));
            }
            if pulse > state.until {
                return Err(ExecError::Invalid("the stream has expired".into()));
            }
            let latest = pulse.0.saturating_add(crate::market::MAX_PERIOD);
            if *until < state.until || until.0 > latest {
                return Err(ExecError::Invalid(format!(
                    "the new last Pulse must be between {} and {latest}",
                    state.until
                )));
            }
            if *deposit == Amount::ZERO && *until == state.until {
                return Err(ExecError::Invalid(
                    "a top-up must add funds or extend the stream".into(),
                ));
            }
            let asset = state.asset;
            if *deposit > Amount::ZERO {
                state.escrow = state.escrow.checked_add(*deposit)?;
            }
            state.until = *until;
            if *deposit > Amount::ZERO {
                run.draw(asset, *deposit)?;
            }
            run.store(object);
            run.event(Event::StreamToppedUp {
                stream: *stream,
                deposit: *deposit,
                until: *until,
            });
        }
        TxKind::CloseStream { stream } => {
            let mut object = run.load(stream)?;
            let ObjectData::Stream(state) = &mut object.data else {
                return Err(ExecError::Invalid(format!("{stream} is not a stream")));
            };
            if !state.open {
                return Err(ExecError::Invalid("the stream is closed".into()));
            }
            let by_payee = state.payee == tx.sender;
            let by_payer_after_expiry = state.payer == tx.sender && pulse > state.until;
            if !by_payee && !by_payer_after_expiry {
                return Err(ExecError::Unauthorized(
                    "only the payee, or the payer once the stream has expired, may close it".into(),
                ));
            }
            let refund = state.escrow;
            let (payer, asset) = (state.payer, state.asset);
            state.escrow = Amount::ZERO;
            state.open = false;
            run.store(object);
            run.mint_coin(payer, asset, refund);
            run.event(Event::StreamClosed {
                stream: *stream,
                refund,
            });
        }
        TxKind::OpenMemory { agent, policy } => {
            if let Some(agent_id) = agent {
                let charter = load_charter(run, agent_id)?;
                if charter.identity.is_none() {
                    return Err(ExecError::Invalid("not an agent identity".into()));
                }
                if charter.agent != tx.sender {
                    return Err(ExecError::Unauthorized(
                        "only the agent may open a memory log for itself".into(),
                    ));
                }
                check_charter_chain(run, agent_id)?;
            }
            let log = created_id(&run.digest, run.next_index);
            run.create(
                Owner::Account(tx.sender),
                ObjectData::MemoryLog(MemoryLog {
                    writer: tx.sender,
                    agent: *agent,
                    policy: *policy,
                    size: 0,
                    root: merkle::empty_root(),
                    index: Hash32::ZERO,
                    manifest: Hash32::ZERO,
                    commits: 0,
                    opened_at: pulse,
                    committed_at: pulse,
                    sealed: false,
                }),
            );
            run.event(Event::MemoryOpened {
                log,
                account: tx.sender,
                agent: *agent,
            });
        }
        TxKind::CommitMemory {
            size,
            root,
            index,
            manifest,
            proof,
            seal,
        } => {
            let log_id = run.input(1)?;
            let mut object = run.load(&log_id)?;
            let ObjectData::MemoryLog(log) = &mut object.data else {
                return Err(ExecError::Invalid("input 1 is not a memory log".into()));
            };
            if log.writer != tx.sender {
                return Err(ExecError::Unauthorized(
                    "only the writer may commit to a memory log".into(),
                ));
            }
            if log.sealed {
                return Err(ExecError::Invalid("the memory log is sealed".into()));
            }
            if let Some(agent) = log.agent {
                check_charter_chain(run, &agent)?;
            }
            if proof.len() > MAX_CONSISTENCY_PROOF {
                return Err(ExecError::Invalid(format!(
                    "a consistency proof has at most {MAX_CONSISTENCY_PROOF} hashes"
                )));
            }
            if *size < log.size {
                return Err(ExecError::Invalid(format!(
                    "a memory log only grows: it holds {} atoms, not {size}",
                    log.size
                )));
            }
            if *size == 0 && *root != merkle::empty_root() {
                return Err(ExecError::Invalid("an empty log has the empty root".into()));
            }
            merkle::verify_consistency(&log.root, log.size, root, *size, proof).map_err(|e| {
                ExecError::Invalid(format!("the new head does not extend the log: {e}"))
            })?;
            log.size = *size;
            log.root = *root;
            log.index = *index;
            log.manifest = *manifest;
            log.commits = log.commits.saturating_add(1);
            log.committed_at = pulse;
            log.sealed = *seal;
            let event = Event::MemoryCommitted {
                log: log_id,
                size: *size,
                root: *root,
                commits: log.commits,
                sealed: *seal,
            };
            run.store(object);
            run.event(event);
        }
    }
    Ok(())
}

/// The record of `validator`, which `operator` must operate.
fn operated<'s>(
    set: &'s mut ValidatorSet,
    validator: &ValidatorId,
    operator: &AccountId,
) -> Result<&'s mut ValidatorRecord, ExecError> {
    let record = set
        .validators
        .get_mut(validator)
        .ok_or_else(|| ExecError::Invalid("unknown validator".into()))?;
    if record.operator != *operator {
        return Err(ExecError::Unauthorized(
            "only the validator's operator may do this".into(),
        ));
    }
    Ok(record)
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
    sion_core::hash::Hash32::from_hex(owner)
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

/// A charter and all its ancestors must be unrevoked and unsuspended
/// (spec §5.13). At most [`MAX_CHARTER_CHAIN`] charters are loaded: if the
/// last of them still has a parent, the check fails. Returns how many
/// charters it loaded.
fn check_charter_chain<S: ObjectStore>(
    run: &mut Run<'_, '_, S>,
    id: &ObjectId,
) -> Result<usize, ExecError> {
    charter_chain(run, id, true)
}

/// The chain check, with or without the suspension test.
fn charter_chain<S: ObjectStore>(
    run: &mut Run<'_, '_, S>,
    id: &ObjectId,
    suspension: bool,
) -> Result<usize, ExecError> {
    let mut current = Some(*id);
    let mut loaded = 0;
    while let Some(charter_id) = current {
        if loaded == MAX_CHARTER_CHAIN {
            return Err(ExecError::Invalid("delegation chain too deep".into()));
        }
        let charter = load_charter(run, &charter_id)?;
        loaded += 1;
        if charter.revoked_at.is_some() {
            return Err(ExecError::Caveat("charter revoked".into()));
        }
        if suspension && charter.suspended_by.is_some() {
            return Err(ExecError::Caveat("charter suspended".into()));
        }
        current = charter.parent;
    }
    Ok(loaded)
}

/// Whether `sender` oversees `charter` (spec §5.13): its backer, its
/// principal, or the agent of a charter above it. Loads parents only when
/// the sender is neither the backer nor the principal, and at most
/// `MAX_CHARTER_CHAIN − 1` of them.
fn is_overseer<S: ObjectStore>(
    run: &mut Run<'_, '_, S>,
    charter: &Charter,
    sender: &AccountId,
) -> Result<bool, ExecError> {
    if *sender == charter.backer || *sender == charter.principal {
        return Ok(true);
    }
    let mut next = charter.parent;
    let mut loaded = 0;
    while let Some(id) = next {
        if loaded == MAX_CHARTER_CHAIN - 1 {
            return Ok(false);
        }
        let ancestor = load_charter(run, &id)?;
        loaded += 1;
        if ancestor.agent == *sender {
            return Ok(true);
        }
        next = ancestor.parent;
    }
    Ok(false)
}

/// Carves a child's window limit out of its parent's allotment (spec
/// §5.13 **carve**): the child's limit must fit in what the parent has not
/// carved yet, and is then carved too.
fn carve_window(
    allotment: &mut Allotment,
    caveats: &Caveats,
    child_limit: &Amount,
) -> Result<(), ExecError> {
    let left = caveats
        .max_per_window
        .checked_sub(allotment.carved_window)?;
    if *child_limit > left {
        return Err(ExecError::Caveat(format!(
            "window allowance left to carve is {left}, below the child's {child_limit}"
        )));
    }
    allotment.carved_window = allotment.carved_window.checked_add(*child_limit)?;
    Ok(())
}

/// Launches one subagent under `parent_id` (spec §5.36 step 8): its
/// charter with an identity at `depth`, and its allotment.
fn spawn_one<S: ObjectStore>(
    run: &mut Run<'_, '_, S>,
    parent_id: &ObjectId,
    parent: &Charter,
    child: &Spawn,
    depth: usize,
) -> Result<(), ExecError> {
    let pulse = run.ctx.pulse;
    let sender = run.tx.sender;
    let depth = u8::try_from(depth).map_err(|_| ExecError::Invalid("too deep".into()))?;
    let child_id = created_id(&run.digest, run.next_index);
    run.create(
        Owner::Shared(object_domain("charter", &child_id)),
        ObjectData::Charter(Charter {
            principal: sender,
            agent: child.agent,
            caveats: child.caveats.clone(),
            agent_atom: parent.agent_atom,
            backer: parent.backer,
            revoked_at: None,
            parent: Some(*parent_id),
            allotments: BTreeSet::new(),
            created_at: pulse,
            suspended_by: None,
            identity: Some(AgentIdentity {
                card: child.card,
                depth,
            }),
        }),
    );
    let allotment = run.create(
        Owner::Account(child.agent),
        ObjectData::Allotment(Allotment {
            source: child_id,
            writer: child.agent,
            asset: AssetId::usd(),
            remaining: child.budget,
            caveats: Some(child.caveats.clone()),
            window_start: pulse,
            spent_in_window: Amount::ZERO,
            carved_window: Amount::ZERO,
        }),
    );
    let mut object = run.load(&child_id)?;
    if let ObjectData::Charter(c) = &mut object.data {
        c.allotments.insert(allotment);
    }
    run.store(object);
    run.event(Event::AgentSpawned {
        agent: child_id,
        parent: *parent_id,
        account: child.agent,
        allotment,
    });
    Ok(())
}

/// Creates a root charter and its allotment (CreateCharter, RegisterAgent):
/// spec §5.12 steps 1 to 7. Returns the charter and the allotment.
fn create_root_charter<S: ObjectStore>(
    run: &mut Run<'_, '_, S>,
    agent: &AccountId,
    caveats: &Caveats,
    funding: Amount,
    agent_bond: Amount,
    identity: Option<AgentIdentity>,
) -> Result<(ObjectId, ObjectId), ExecError> {
    let pulse = run.ctx.pulse;
    let sender = run.tx.sender;
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
    run.draw(AssetId::usd(), funding.checked_add(agent_bond)?)?;
    let charter_id = created_id(&run.digest, run.next_index);
    let label = format!("charter:{charter_id}");
    let agent_atom = bond_atom(AtomClass::Economic, &sender, &label);
    let domain = object_domain("charter", &charter_id);
    let created = run.create(
        Owner::Shared(domain),
        ObjectData::Charter(Charter {
            principal: sender,
            agent: *agent,
            caveats: caveats.clone(),
            agent_atom: agent_atom.id,
            backer: sender,
            revoked_at: None,
            parent: None,
            allotments: BTreeSet::new(),
            created_at: pulse,
            suspended_by: None,
            identity,
        }),
    );
    debug_assert_eq!(created, charter_id);
    let allotment = run.create(
        Owner::Account(*agent),
        ObjectData::Allotment(Allotment {
            source: charter_id,
            writer: *agent,
            asset: AssetId::usd(),
            remaining: funding,
            caveats: Some(caveats.clone()),
            window_start: pulse,
            spent_in_window: Amount::ZERO,
            carved_window: Amount::ZERO,
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
    if agent_bond > Amount::ZERO {
        run.effects.deferred.push(Deferred::PostBond {
            atom: agent_atom,
            owner: sender,
            amount: agent_bond,
        });
    }
    Ok((charter_id, allotment))
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
