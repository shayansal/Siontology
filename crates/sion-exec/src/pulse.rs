//! Executing a Pulse.
//!
//! A Pulse runs in three phases:
//!
//! 1. **Owned**: certified owned-path transactions (linear, bounded and
//!    knowledge classes). Each owned input is locked to one transaction by
//!    its lane certificate, and no owned-path operation writes a pre-existing
//!    shared object, so these transactions commute: any order that respects
//!    creation dependencies gives the same state. The canonical order is
//!    digest order, with a transaction whose inputs are created by another
//!    transaction of the same Pulse deferred until they exist
//!    ([`canonical_order`]).
//! 2. **Contended**: consensus-ordered transactions, in commit order.
//! 3. **Deferred**: the commutative effects collected from phases 1 and 2 —
//!    atoms, bonds, claims (sorted by claim ID), orders, returns, fees,
//!    schedule entries — then the forced intents posted in this Pulse, then
//!    the events the schedule says are due (batch sealing, clearing
//!    deadlines, registry ticks, forced-inclusion deadlines, the end of an
//!    epoch). Claims may only reference claims from earlier Pulses, so the
//!    knowledge phase never depends on the order of transactions within a
//!    Pulse (CALM).
//!
//! The same code produces *chunk traces* for the Fold: the Pulse is cut into
//! consecutive chunks of transactions plus the deferred phase, each with the
//! state commitment before and after and a witness (the touched objects with
//! sparse-Merkle proofs against the chunk's starting root). A prover can
//! re-execute each chunk statelessly and in parallel
//! ([`crate::stateless::replay_chunk`]).

use std::collections::{BTreeMap, BTreeSet};

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sion_assurance::{ClaimRecord, ClaimStatus, Ratio, Registry};
use sion_consensus::resample_committee;
use sion_core::encode::encode;
use sion_core::hash::{hash_domain, hash_parts, Hash32};
use sion_core::ids::{
    AccountId, AssetId, ClaimId, Epoch, ObjectId, Pulse, ShardId, TxDigest, Version,
};
use sion_core::merkle::{self, Frontier};
use sion_core::smt::{SmtProof, SmtSnapshot, SparseMerkleTree};
use sion_core::Amount;

use crate::assurance::AssuranceDomain;
use crate::auth::{check_authorization, find_authority};
use crate::claim::{claim_id, knowledge_leaf, ClaimBody, StoredClaim};
use crate::delegation::{mul_div, DelegationBook, Join, REWARD_SCALE};
use crate::error::ExecError;
use crate::execute::{
    bond_owner, execute_transaction, Deferred, Event, ExecStatus, PulseContext, TxEffects,
};
use crate::market::{BatchStatus, Market, MarketBatch};
use crate::object::{
    well_known, Coin, Object, ObjectData, Owner, Schedule, ScheduledEvent, Treasury,
};
use crate::state::{ChainConfig, State, WorldCommitment};
use crate::store::{ObjectStore, Overlay};
use crate::tx::{required_fee, SignedTransaction, TxKind, MAX_INPUTS};
use crate::validators::{
    rotation_seed, share_of, weight, ForcedObligations, Obligation, ShardRotation, ValidatorSet,
    ValidatorStatus,
};

/// What a Pulse executes.
#[derive(
    Clone, Debug, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub struct PulseInput {
    /// The Pulse number (one more than the state's).
    pub pulse: Pulse,
    /// The beacon value for this Pulse.
    pub beacon: Hash32,
    /// Certified owned-path transactions, in any order.
    pub owned: Vec<SignedTransaction>,
    /// Consensus-ordered contended transactions.
    pub contended: Vec<SignedTransaction>,
    /// Forced intents committed in this Pulse, in commit order. They are not
    /// executed here: each becomes an obligation of its sender's shard.
    pub forced: Vec<SignedTransaction>,
}

/// Digest of a Pulse's executed order (bound into the Pulse header).
pub fn order_digest(pulse: Pulse, beacon: &Hash32, order: &[TxDigest]) -> Hash32 {
    hash_domain("sion/exec/pulse-order", &encode(&(pulse, beacon, order)))
}

/// What a Pulse produced.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PulseOutput {
    /// The Pulse.
    pub pulse: Pulse,
    /// The beacon.
    pub beacon: Hash32,
    /// Commitment before.
    pub before: WorldCommitment,
    /// Commitment after.
    pub after: WorldCommitment,
    /// Execution order (owned phase, then contended phase).
    pub order: Vec<TxDigest>,
    /// Digest of the order.
    pub order_digest: Hash32,
    /// Per-transaction effects, in execution order.
    pub effects: Vec<TxEffects>,
    /// Events from the deferred phase.
    pub deferred_events: Vec<Event>,
    /// Claims that joined the knowledge log.
    pub new_claims: Vec<StoredClaim>,
    /// Merkle root over the effects (RFC 9162 tree over [`effects_leaf`]).
    pub receipts_root: Hash32,
}

impl PulseOutput {
    /// Counts of successful, failed and rejected transactions.
    pub fn status_counts(&self) -> (usize, usize, usize) {
        status_counts(&self.effects)
    }
}

/// The outcome code committed for a transaction: 0 success, 1 failed,
/// 2 rejected. The human-readable reason is reported but not committed, so
/// independent implementations agree on receipts without agreeing on prose.
pub fn status_code(status: &ExecStatus) -> u8 {
    match status {
        ExecStatus::Success => 0,
        ExecStatus::Failed(_) => 1,
        ExecStatus::Rejected(_) => 2,
    }
}

/// The Merkle leaf for one transaction's effects: every field of the
/// effects, with the status reduced to its [`status_code`].
pub fn effects_leaf(effects: &TxEffects) -> Hash32 {
    merkle::leaf_hash(&encode(&(
        effects.digest,
        effects.sender,
        effects.class,
        status_code(&effects.status),
        effects.fee,
        &effects.created,
        &effects.mutated,
        &effects.consumed,
        &effects.events,
        &effects.deferred,
    )))
}

/// The witness for one chunk: the touched objects as they were before the
/// chunk, each with a sparse-Merkle proof (`None` = absent), and the
/// knowledge log's frontier.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Witness {
    /// Chain configuration (checked against the commitment's digest).
    pub config: ChainConfig,
    /// Touched objects and their proofs, by identifier.
    pub objects: Vec<(ObjectId, Option<Object>, SmtProof)>,
    /// The knowledge log's frontier before the chunk.
    pub frontier: Frontier,
}

impl Witness {
    /// Encoded size in bytes.
    pub fn size(&self) -> usize {
        encode(self).len()
    }
}

/// The statement one chunk proves: executing these steps from `before`
/// reaches `after`, extending the deferred-effects and receipts
/// accumulators.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
    Deserialize,
)]
pub struct ChunkStatement {
    /// The Pulse.
    pub pulse: Pulse,
    /// The Pulse's beacon.
    pub beacon: Hash32,
    /// Position of the chunk in the Pulse.
    pub index: u32,
    /// Whether this is the deferred phase (always the last chunk).
    pub deferred_phase: bool,
    /// Commitment before.
    pub before: WorldCommitment,
    /// Commitment after.
    pub after: WorldCommitment,
    /// Deferred-effects accumulator before. For the deferred phase: the
    /// accumulator the transaction chunks produced.
    pub deferred_in: Hash32,
    /// Deferred-effects accumulator after. For the deferred phase: the
    /// accumulator recomputed over the items it consumed (so the two must
    /// be equal).
    pub deferred_out: Hash32,
    /// Receipts accumulator before.
    pub effects_in: Hash32,
    /// Receipts accumulator after.
    pub effects_out: Hash32,
    /// Digest of the transactions executed, in order.
    pub steps: Hash32,
}

impl ChunkStatement {
    /// Digest of the statement.
    pub fn digest(&self) -> Hash32 {
        hash_domain("sion/exec/chunk-statement", &encode(self))
    }
}

/// One chunk: the statement plus what is needed to check it.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct ChunkTrace {
    /// The statement.
    pub statement: ChunkStatement,
    /// Transactions executed; for the deferred phase, the forced intents
    /// posted in the Pulse.
    pub transactions: Vec<SignedTransaction>,
    /// For the deferred phase: every deferred item of the Pulse, in the
    /// order the transactions produced them.
    pub deferred_items: Vec<Deferred>,
    /// The witness.
    pub witness: Witness,
    /// The chunk's writes, by identifier.
    pub writes: Vec<Object>,
    /// Effects produced (empty for the deferred phase).
    pub effects: Vec<TxEffects>,
}

/// Extends the deferred accumulator by `items`, one at a time.
pub fn chain_deferred(acc: &Hash32, items: &[Deferred]) -> Hash32 {
    items.iter().fold(*acc, |acc, item| {
        hash_parts("sion/exec/deferred-acc", &[&acc.0, &encode(item)])
    })
}

/// Extends the receipts accumulator by one transaction's effects.
pub fn chain_effects(acc: &Hash32, effects: &TxEffects) -> Hash32 {
    chain_effects_leaf(acc, &effects_leaf(effects))
}

/// Extends the receipts accumulator by one effects leaf.
pub fn chain_effects_leaf(acc: &Hash32, leaf: &Hash32) -> Hash32 {
    hash_parts("sion/exec/effects-acc", &[&acc.0, &leaf.0])
}

/// Digest of an ordered list of transactions.
pub fn steps_digest(transactions: &[SignedTransaction]) -> Hash32 {
    let digests: Vec<TxDigest> = transactions.iter().map(|t| t.digest()).collect();
    steps_digest_of(&digests)
}

/// Digest of an ordered list of transaction digests (see [`steps_digest`]).
pub fn steps_digest_of(digests: &[TxDigest]) -> Hash32 {
    hash_domain("sion/exec/steps", &encode(&digests.to_vec()))
}

/// Status counts: (success, failed, rejected).
pub fn status_counts(effects: &[TxEffects]) -> (usize, usize, usize) {
    let mut counts = (0, 0, 0);
    for e in effects {
        match e.status {
            ExecStatus::Success => counts.0 += 1,
            ExecStatus::Failed(_) => counts.1 += 1,
            ExecStatus::Rejected(_) => counts.2 += 1,
        }
    }
    counts
}

/// A base map with a layer of newer writes on top.
struct Layered<'a> {
    base: &'a BTreeMap<ObjectId, Object>,
    top: &'a BTreeMap<ObjectId, Object>,
}

impl ObjectStore for Layered<'_> {
    fn get_object(&self, id: &ObjectId) -> Result<Option<Object>, ExecError> {
        Ok(self.top.get(id).or_else(|| self.base.get(id)).cloned())
    }
}

impl Layered<'_> {
    fn lookup(&self, id: &ObjectId) -> Option<Object> {
        self.top.get(id).or_else(|| self.base.get(id)).cloned()
    }
}

/// The forced intents a Pulse posts: those committed, in commit order, each
/// once.
pub fn posted_intents(forced: &[SignedTransaction]) -> Vec<SignedTransaction> {
    let mut seen = BTreeSet::new();
    forced
        .iter()
        .filter(|tx| seen.insert(tx.digest()))
        .cloned()
        .collect()
}

/// The canonical execution order: owned transactions in digest order,
/// repeatedly, each pass executing those whose inputs exist (so a
/// transaction spending an output created earlier in the same Pulse runs
/// after its creator), then contended transactions in consensus order.
/// Duplicates are dropped; a transaction that needs consensus but arrived on
/// the owned path makes the whole input invalid.
///
/// The passes are computed by a dry run without signature checks. The order
/// only decides which dependent transactions find their inputs, never what a
/// transaction may do, so proofs attest to the execution of the order the
/// committee committed to.
pub fn canonical_order(
    state: &State,
    input: &PulseInput,
) -> Result<Vec<SignedTransaction>, ExecError> {
    let mut seen = BTreeSet::new();
    let mut owned: Vec<(TxDigest, &SignedTransaction)> = Vec::new();
    for tx in &input.owned {
        if tx.tx.domain().is_some() {
            return Err(ExecError::Invalid(format!(
                "transaction {} needs consensus but was submitted on the owned path",
                tx.digest()
            )));
        }
        let digest = tx.digest();
        if seen.insert(digest) {
            owned.push((digest, tx));
        }
    }
    owned.sort_by_key(|(digest, _)| *digest);
    let dry = PulseContext {
        pulse: input.pulse,
        beacon: input.beacon,
        config: state.config.clone(),
        verify_signatures: false,
    };
    let mut overlay = Overlay::new(&state.objects);
    let mut order = Vec::with_capacity(owned.len() + input.contended.len());
    let mut pending = owned;
    loop {
        let mut progressed = false;
        let mut waiting = Vec::new();
        for (digest, tx) in pending {
            let ready = tx
                .tx
                .inputs
                .iter()
                .all(|input| matches!(overlay.get(&input.id), Ok(Some(_))));
            if ready {
                execute_transaction(&mut overlay, &dry, tx);
                order.push(tx.clone());
                progressed = true;
            } else {
                waiting.push((digest, tx));
            }
        }
        pending = waiting;
        if pending.is_empty() || !progressed {
            break;
        }
    }
    // Whatever never became ready runs last and is rejected for its missing inputs.
    order.extend(pending.into_iter().map(|(_, tx)| tx.clone()));
    for tx in &input.contended {
        if seen.insert(tx.digest()) {
            order.push(tx.clone());
        }
    }
    Ok(order)
}

fn build_witness(
    config: &ChainConfig,
    store: &Layered<'_>,
    snapshot: &SmtSnapshot,
    touched: &BTreeSet<ObjectId>,
    frontier: &Frontier,
) -> Witness {
    Witness {
        config: config.clone(),
        objects: touched
            .iter()
            .map(|id| (*id, store.lookup(id), snapshot.prove(&id.0)))
            .collect(),
        frontier: frontier.clone(),
    }
}

fn update_tree(tree: &mut SparseMerkleTree, writes: &BTreeMap<ObjectId, Object>) {
    for (id, object) in writes {
        tree.insert(id.0, object.digest());
    }
}

/// Executes a Pulse on a full state. On error the state is unchanged.
pub fn execute_pulse(
    state: &mut State,
    input: &PulseInput,
    verify_signatures: bool,
) -> Result<PulseOutput, ExecError> {
    run_pulse(state, input, verify_signatures, None).map(|(output, _)| output)
}

/// Executes a Pulse and returns its chunk traces: `chunk_size` transactions
/// per chunk, plus one chunk for the deferred phase.
pub fn execute_pulse_traced(
    state: &mut State,
    input: &PulseInput,
    verify_signatures: bool,
    chunk_size: usize,
) -> Result<(PulseOutput, Vec<ChunkTrace>), ExecError> {
    run_pulse(state, input, verify_signatures, Some(chunk_size.max(1)))
}

fn run_pulse(
    state: &mut State,
    input: &PulseInput,
    verify_signatures: bool,
    chunking: Option<usize>,
) -> Result<(PulseOutput, Vec<ChunkTrace>), ExecError> {
    if input.pulse != state.pulse.next() {
        return Err(ExecError::Invalid(format!(
            "expected Pulse {}, got {}",
            state.pulse.next(),
            input.pulse
        )));
    }
    let ctx = PulseContext {
        pulse: input.pulse,
        beacon: input.beacon,
        config: state.config.clone(),
        verify_signatures,
    };
    let before = state.commitment();
    let ordered = canonical_order(state, input)?;
    let forced = posted_intents(&input.forced);
    let tracing = chunking.is_some();
    let chunk_size = chunking.unwrap_or(usize::MAX);

    let mut pulse_writes: BTreeMap<ObjectId, Object> = BTreeMap::new();
    let mut tree = state.tree.clone();
    let mut frontier = state.knowledge.frontier();
    let mut commitment = before;
    let mut deferred_acc = Hash32::ZERO;
    let mut effects_acc = Hash32::ZERO;
    let mut all_deferred: Vec<Deferred> = Vec::new();
    let mut all_effects: Vec<TxEffects> = Vec::with_capacity(ordered.len());
    let mut traces = Vec::new();

    let mut chunks: Vec<&[SignedTransaction]> = ordered.chunks(chunk_size).collect();
    if chunks.is_empty() && tracing {
        chunks.push(&[]);
    }
    for (index, chunk) in chunks.iter().enumerate() {
        let snapshot = tracing.then(|| tree.snapshot());
        let (witness, writes, chunk_effects) = {
            let layered = Layered {
                base: &state.objects,
                top: &pulse_writes,
            };
            let mut overlay = Overlay::new(&layered);
            let effects: Vec<TxEffects> = chunk
                .iter()
                .map(|tx| execute_transaction(&mut overlay, &ctx, tx))
                .collect();
            let witness = snapshot.as_ref().map(|snapshot| {
                build_witness(
                    &ctx.config,
                    &layered,
                    snapshot,
                    &overlay.touched(),
                    &frontier,
                )
            });
            (witness, overlay.into_writes(), effects)
        };
        let deferred_in = deferred_acc;
        let effects_in = effects_acc;
        for effects in &chunk_effects {
            deferred_acc = chain_deferred(&deferred_acc, &effects.deferred);
            effects_acc = chain_effects(&effects_acc, effects);
            all_deferred.extend(effects.deferred.iter().cloned());
        }
        update_tree(&mut tree, &writes);
        if let Some(witness) = witness {
            let after = WorldCommitment {
                pulse: input.pulse,
                state_root: tree.root(),
                knowledge_root: frontier.root(),
                knowledge_size: frontier.size,
                config: before.config,
            };
            traces.push(ChunkTrace {
                statement: ChunkStatement {
                    pulse: input.pulse,
                    beacon: input.beacon,
                    index: index as u32,
                    deferred_phase: false,
                    before: commitment,
                    after,
                    deferred_in,
                    deferred_out: deferred_acc,
                    effects_in,
                    effects_out: effects_acc,
                    steps: steps_digest(chunk),
                },
                transactions: chunk.to_vec(),
                deferred_items: Vec::new(),
                witness,
                writes: writes.values().cloned().collect(),
                effects: chunk_effects.clone(),
            });
            commitment = after;
        }
        pulse_writes.extend(writes);
        all_effects.extend(chunk_effects);
    }

    // The deferred phase.
    let snapshot = tracing.then(|| tree.snapshot());
    let frontier_before = frontier.clone();
    let (witness, writes, outcome) = {
        let layered = Layered {
            base: &state.objects,
            top: &pulse_writes,
        };
        let mut overlay = Overlay::new(&layered);
        let outcome = apply_deferred(&mut overlay, &ctx, &all_deferred, &forced, &mut frontier)?;
        let witness = snapshot.as_ref().map(|snapshot| {
            build_witness(
                &ctx.config,
                &layered,
                snapshot,
                &overlay.touched(),
                &frontier_before,
            )
        });
        (witness, overlay.into_writes(), outcome)
    };
    update_tree(&mut tree, &writes);
    let after = WorldCommitment {
        pulse: input.pulse,
        state_root: tree.root(),
        knowledge_root: frontier.root(),
        knowledge_size: frontier.size,
        config: before.config,
    };
    if let Some(witness) = witness {
        traces.push(ChunkTrace {
            statement: ChunkStatement {
                pulse: input.pulse,
                beacon: input.beacon,
                index: traces.len() as u32,
                deferred_phase: true,
                before: commitment,
                after,
                deferred_in: deferred_acc,
                deferred_out: chain_deferred(&Hash32::ZERO, &all_deferred),
                effects_in: effects_acc,
                effects_out: effects_acc,
                steps: steps_digest(&forced),
            },
            transactions: forced.clone(),
            deferred_items: all_deferred,
            witness,
            writes: writes.values().cloned().collect(),
            effects: Vec::new(),
        });
    }
    pulse_writes.extend(writes);

    // Commit.
    state.objects.extend(pulse_writes);
    state.tree = tree;
    for claim in &outcome.new_claims {
        state.knowledge.append(knowledge_leaf(claim));
        state.claims.insert(claim.id, claim.clone());
    }
    state.pulse = input.pulse;
    debug_assert_eq!(state.knowledge.root(), after.knowledge_root);

    let leaves: Vec<Hash32> = all_effects.iter().map(effects_leaf).collect();
    let order: Vec<TxDigest> = ordered.iter().map(|t| t.digest()).collect();
    let output = PulseOutput {
        pulse: input.pulse,
        beacon: input.beacon,
        before,
        after,
        order_digest: order_digest(input.pulse, &input.beacon, &order),
        order,
        effects: all_effects,
        deferred_events: outcome.events,
        new_claims: outcome.new_claims,
        receipts_root: merkle::root(&leaves),
    };
    Ok((output, traces))
}

/// What the deferred phase produced.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeferredOutcome {
    /// Events.
    pub events: Vec<Event>,
    /// Claims appended to the knowledge log.
    pub new_claims: Vec<StoredClaim>,
}

/// Writes made by the deferred phase, versioned like a transaction's.
struct DeferredRun<'r, 'a, S: ObjectStore> {
    overlay: &'r mut Overlay<'a, S>,
    pulse: Pulse,
    originals: BTreeMap<ObjectId, Version>,
    created: BTreeSet<ObjectId>,
    counter: u64,
}

impl<S: ObjectStore> DeferredRun<'_, '_, S> {
    fn load(&mut self, id: &ObjectId) -> Result<Option<Object>, ExecError> {
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
        } else {
            object.version = Version(0);
            self.created.insert(object.id);
        }
        self.overlay.put(object);
    }

    /// Pays `amount` of `asset` to `owner` as a new coin.
    fn pay(&mut self, owner: AccountId, asset: AssetId, amount: Amount) {
        if amount == Amount::ZERO {
            return;
        }
        let id = deferred_coin_id(self.pulse, self.counter);
        self.counter += 1;
        self.store(Object {
            id,
            version: Version(0),
            owner: Owner::Account(owner),
            data: ObjectData::Coin(Coin { asset, amount }),
        });
    }
}

/// The identifier of the `counter`-th coin the deferred phase of `pulse` pays out.
pub fn deferred_coin_id(pulse: Pulse, counter: u64) -> ObjectId {
    ObjectId::derive_parts(&[b"deferred", &pulse.0.to_be_bytes(), &counter.to_be_bytes()])
}

/// Protocol objects the deferred phase reads lazily and writes back only if
/// it changed them, so a quiet Pulse touches only the schedule.
struct Protocol {
    assurance: Option<(Object, AssuranceDomain, bool)>,
    treasury: Option<(Object, Treasury, bool)>,
    validators: Option<(Object, ValidatorSet, bool)>,
    /// Not loaded yet, or loaded: `None` if no delegation created the book.
    delegations: Option<Option<(Object, DelegationBook, bool)>>,
    forced: Option<(Object, ForcedObligations, bool)>,
    schedule: (Object, Schedule, bool),
    markets: BTreeMap<ObjectId, (Object, bool)>,
}

impl Protocol {
    fn assurance<S: ObjectStore>(
        &mut self,
        run: &mut DeferredRun<'_, '_, S>,
    ) -> Result<&mut AssuranceDomain, ExecError> {
        if self.assurance.is_none() {
            let object = run
                .load(&well_known::registry())?
                .ok_or(ExecError::MissingObject(well_known::registry()))?;
            let ObjectData::Assurance(domain) = &object.data else {
                return Err(ExecError::Invalid("registry object corrupted".into()));
            };
            let domain = (**domain).clone();
            self.assurance = Some((object, domain, false));
        }
        let (_, domain, dirty) = self.assurance.as_mut().expect("loaded above");
        *dirty = true;
        Ok(domain)
    }

    fn treasury<S: ObjectStore>(
        &mut self,
        run: &mut DeferredRun<'_, '_, S>,
    ) -> Result<&mut Treasury, ExecError> {
        if self.treasury.is_none() {
            let object = run
                .load(&well_known::treasury())?
                .ok_or(ExecError::MissingObject(well_known::treasury()))?;
            let ObjectData::Treasury(treasury) = &object.data else {
                return Err(ExecError::Invalid("treasury corrupted".into()));
            };
            let treasury = treasury.clone();
            self.treasury = Some((object, treasury, false));
        }
        let (_, treasury, dirty) = self.treasury.as_mut().expect("loaded above");
        *dirty = true;
        Ok(treasury)
    }

    fn validators<S: ObjectStore>(
        &mut self,
        run: &mut DeferredRun<'_, '_, S>,
    ) -> Result<&mut ValidatorSet, ExecError> {
        if self.validators.is_none() {
            let object = run
                .load(&well_known::validators())?
                .ok_or(ExecError::MissingObject(well_known::validators()))?;
            let ObjectData::Validators(set) = &object.data else {
                return Err(ExecError::Invalid("validator set corrupted".into()));
            };
            let set = (**set).clone();
            self.validators = Some((object, set, false));
        }
        let (_, set, dirty) = self.validators.as_mut().expect("loaded above");
        *dirty = true;
        Ok(set)
    }

    /// The validator set and the delegation book, if one exists.
    fn staking<S: ObjectStore>(
        &mut self,
        run: &mut DeferredRun<'_, '_, S>,
    ) -> Result<(&mut ValidatorSet, Option<&mut DelegationBook>), ExecError> {
        self.validators(run)?;
        if self.delegations.is_none() {
            let loaded = match run.load(&well_known::delegations())? {
                Some(object) => {
                    let ObjectData::Delegations(book) = &object.data else {
                        return Err(ExecError::Invalid("delegation book corrupted".into()));
                    };
                    let book = (**book).clone();
                    Some((object, book, false))
                }
                None => None,
            };
            self.delegations = Some(loaded);
        }
        let (_, set, _) = self.validators.as_mut().expect("loaded above");
        let book = match self.delegations.as_mut().expect("loaded above") {
            Some((_, book, dirty)) => {
                *dirty = true;
                Some(book)
            }
            None => None,
        };
        Ok((set, book))
    }

    fn forced<S: ObjectStore>(
        &mut self,
        run: &mut DeferredRun<'_, '_, S>,
    ) -> Result<&mut ForcedObligations, ExecError> {
        if self.forced.is_none() {
            let object = run
                .load(&well_known::forced())?
                .ok_or(ExecError::MissingObject(well_known::forced()))?;
            let ObjectData::Forced(obligations) = &object.data else {
                return Err(ExecError::Invalid("forced obligations corrupted".into()));
            };
            let obligations = (**obligations).clone();
            self.forced = Some((object, obligations, false));
        }
        let (_, obligations, dirty) = self.forced.as_mut().expect("loaded above");
        *dirty = true;
        Ok(obligations)
    }

    fn market<S: ObjectStore>(
        &mut self,
        run: &mut DeferredRun<'_, '_, S>,
        id: &ObjectId,
    ) -> Result<Option<&mut Market>, ExecError> {
        if !self.markets.contains_key(id) {
            let Some(object) = run.load(id)? else {
                return Ok(None);
            };
            if !matches!(object.data, ObjectData::Market(_)) {
                return Ok(None);
            }
            self.markets.insert(*id, (object, false));
        }
        let (object, dirty) = self.markets.get_mut(id).expect("inserted above");
        let ObjectData::Market(market) = &mut object.data else {
            return Ok(None);
        };
        *dirty = true;
        Ok(Some(market))
    }

    fn schedule_event(&mut self, at: Pulse, event: ScheduledEvent) {
        let (_, schedule, dirty) = &mut self.schedule;
        if schedule.due.entry(at.0).or_default().insert(event) {
            *dirty = true;
        }
    }

    fn write_back<S: ObjectStore>(self, run: &mut DeferredRun<'_, '_, S>) {
        for (_, (object, dirty)) in self.markets {
            if dirty {
                run.store(object);
            }
        }
        if let Some((mut object, domain, true)) = self.assurance {
            object.data = ObjectData::Assurance(Box::new(domain));
            run.store(object);
        }
        if let Some((mut object, treasury, true)) = self.treasury {
            object.data = ObjectData::Treasury(treasury);
            run.store(object);
        }
        if let Some((mut object, set, true)) = self.validators {
            object.data = ObjectData::Validators(Box::new(set));
            run.store(object);
        }
        if let Some(Some((mut object, book, true))) = self.delegations {
            object.data = ObjectData::Delegations(Box::new(book));
            run.store(object);
        }
        if let Some((mut object, obligations, true)) = self.forced {
            object.data = ObjectData::Forced(Box::new(obligations));
            run.store(object);
        }
        let (mut object, schedule, dirty) = self.schedule;
        if dirty {
            object.data = ObjectData::Schedule(schedule);
            run.store(object);
        }
    }
}

/// Sort key of a deferred item: (kind rank, natural key, encoding).
type DeferredKey = (u8, Vec<u8>, Vec<u8>);

/// The canonical position of a deferred item: by kind (atoms, bonds,
/// claims, orders, returns, treasury, mint records, schedule entries), then
/// by the item's natural key (claims by claim ID, orders by order ID), then
/// by its encoding.
fn deferred_key(item: &Deferred) -> DeferredKey {
    let (rank, key) = match item {
        Deferred::RegisterAtom { atom } => (0, encode(&atom.id)),
        Deferred::PostBond { atom, owner, .. } => (1, encode(&(atom.id, owner))),
        Deferred::IssueClaim { issuer, body, .. } => (2, encode(&claim_id(issuer, body))),
        Deferred::AddOrder { order, .. } => (3, encode(&order.id)),
        Deferred::ReturnToBounded { bounded, .. } => (4, encode(bounded)),
        Deferred::Treasury { asset, .. } => (5, encode(asset)),
        Deferred::Minted { asset, .. } => (6, encode(asset)),
        Deferred::Schedule { at, event } => (7, encode(&(at, event))),
        Deferred::Fee { asset, .. } => (8, encode(asset)),
    };
    (rank, key, encode(item))
}

/// Applies the deferred phase: commutative effects in canonical order, then
/// the forced intents posted in this Pulse (`forced`, in commit order), then
/// the scheduled events due at this Pulse.
pub fn apply_deferred<S: ObjectStore>(
    overlay: &mut Overlay<'_, S>,
    ctx: &PulseContext,
    items: &[Deferred],
    forced: &[SignedTransaction],
    frontier: &mut Frontier,
) -> Result<DeferredOutcome, ExecError> {
    let mut run = DeferredRun {
        overlay,
        pulse: ctx.pulse,
        originals: BTreeMap::new(),
        created: BTreeSet::new(),
        counter: 0,
    };
    let schedule_object = run
        .load(&well_known::schedule())?
        .ok_or(ExecError::MissingObject(well_known::schedule()))?;
    let ObjectData::Schedule(schedule) = &schedule_object.data else {
        return Err(ExecError::Invalid("schedule corrupted".into()));
    };
    let schedule = schedule.clone();
    let mut protocol = Protocol {
        assurance: None,
        treasury: None,
        validators: None,
        delegations: None,
        forced: None,
        schedule: (schedule_object, schedule, false),
        markets: BTreeMap::new(),
    };
    let mut outcome = DeferredOutcome::default();

    let mut sorted: Vec<(DeferredKey, &Deferred)> =
        items.iter().map(|i| (deferred_key(i), i)).collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));

    for (_, item) in sorted {
        match item {
            Deferred::RegisterAtom { atom } => {
                let domain = protocol.assurance(&mut run)?;
                if domain.registry.atom(&atom.id).is_none()
                    && domain.registry.register_atom(atom.clone()).is_ok()
                {
                    outcome.events.push(Event::AtomRegistered { atom: atom.id });
                }
            }
            Deferred::PostBond {
                atom,
                owner,
                amount,
            } => {
                let domain = protocol.assurance(&mut run)?;
                // The vault must be able to hold the bond before it is posted.
                let vault = domain.vault.checked_add(*amount).ok();
                let registered = vault.is_some()
                    && match domain.registry.atom(&atom.id) {
                        Some(existing) => existing == atom,
                        None => domain.registry.register_atom(atom.clone()).is_ok(),
                    };
                let posted = registered
                    && domain
                        .registry
                        .post_bond(atom.id, *owner, *amount, ctx.pulse)
                        .is_ok();
                if let (true, Some(vault)) = (posted, vault) {
                    domain.vault = vault;
                    outcome.events.push(Event::BondPosted {
                        atom: atom.id,
                        owner: *owner,
                        amount: *amount,
                    });
                } else {
                    // The funds were drawn when the transaction ran; return them.
                    run.pay(*owner, AssetId::usd(), *amount);
                }
            }
            Deferred::IssueClaim {
                issuer,
                backer,
                body,
            } => {
                let id = claim_id(issuer, body);
                let domain = protocol.assurance(&mut run)?;
                match issue(&mut domain.registry, ctx.pulse, *issuer, *backer, id, body) {
                    Ok(valid_until) => {
                        let stored = StoredClaim {
                            id,
                            issuer: *issuer,
                            body: body.clone(),
                            issued_at: ctx.pulse,
                            log_index: frontier.size,
                        };
                        frontier.append(knowledge_leaf(&stored));
                        protocol.schedule_event(valid_until, ScheduledEvent::RegistryTick);
                        outcome.events.push(Event::ClaimIssued {
                            claim: id,
                            log_index: stored.log_index,
                        });
                        outcome.new_claims.push(stored);
                    }
                    Err(reason) => outcome
                        .events
                        .push(Event::ClaimRejected { claim: id, reason }),
                }
            }
            Deferred::AddOrder { market, order } => {
                let pulse = ctx.pulse;
                let added = match protocol.market(&mut run, market)? {
                    None => None,
                    Some(state) => {
                        let index = state.batch_index(pulse);
                        let seal_at = index
                            .saturating_add(1)
                            .saturating_mul(state.batch_length.max(1))
                            - 1;
                        let batch = state.batches.entry(index).or_insert_with(|| MarketBatch {
                            index,
                            orders: Vec::new(),
                            status: BatchStatus::Open,
                        });
                        if batch.status == BatchStatus::Open
                            && batch.orders.len() < crate::market::MAX_BATCH_ORDERS
                        {
                            let first = batch.orders.is_empty();
                            let position = batch.orders.partition_point(|o| o.id < order.id);
                            batch.orders.insert(position, order.clone());
                            Some((index, seal_at, first))
                        } else {
                            None
                        }
                    }
                };
                match added {
                    Some((index, seal_at, true)) => protocol.schedule_event(
                        Pulse(seal_at),
                        ScheduledEvent::SealBatch {
                            market: *market,
                            batch: index,
                        },
                    ),
                    Some(_) => {}
                    None => run.pay(order.owner, order.escrow_asset, order.escrow),
                }
            }
            Deferred::ReturnToBounded { bounded, amount } => {
                if let Some(mut object) = run.load(bounded)? {
                    if let ObjectData::Bounded(state) = &mut object.data {
                        state.returned = state.returned.saturating_add(*amount);
                        state.unallocated = state.unallocated.checked_add(*amount)?;
                        run.store(object);
                    }
                }
            }
            Deferred::Treasury { asset, amount } => {
                let treasury = protocol.treasury(&mut run)?;
                let entry = treasury.balances.entry(*asset).or_insert(Amount::ZERO);
                *entry = entry.saturating_add(*amount);
            }
            Deferred::Minted { asset, amount } => {
                let treasury = protocol.treasury(&mut run)?;
                let entry = treasury.minted.entry(*asset).or_insert(Amount::ZERO);
                *entry = entry.saturating_add(*amount);
            }
            Deferred::Schedule { at, event } => {
                protocol.schedule_event(Pulse(at.0.max(ctx.pulse.0)), *event);
            }
            Deferred::Fee { asset, amount } => {
                let treasury = protocol.treasury(&mut run)?;
                let entry = treasury.balances.entry(*asset).or_insert(Amount::ZERO);
                *entry = entry.saturating_add(*amount);
                let total = treasury.fees.entry(*asset).or_insert(Amount::ZERO);
                *total = total.saturating_add(*amount);
            }
        }
    }

    // Forced intents posted in this Pulse become obligations of their shard.
    for intent in forced {
        let digest = intent.digest();
        let (window, max_pending, known, pending) = {
            let obligations = protocol.forced(&mut run)?;
            (
                obligations.params.window.max(1),
                obligations.params.max_pending,
                obligations.pending.contains_key(&digest),
                obligations.pending.len(),
            )
        };
        let verdict = if known {
            Err("already pending".to_string())
        } else if pending >= max_pending as usize {
            Err("too many intents pending".to_string())
        } else {
            admissible(&mut run, ctx, intent)?
        };
        match verdict {
            Err(reason) => outcome.events.push(Event::ForcedDismissed {
                intent: digest,
                reason,
            }),
            Ok((signer, rotations)) => {
                let tx = &intent.tx;
                let shard = ShardId::for_account(&tx.sender, ctx.config.shard_count.max(1)).0;
                let deadline = Pulse(ctx.pulse.0.saturating_add(window).min(tx.expires_at.0));
                protocol.forced(&mut run)?.pending.insert(
                    digest,
                    Obligation {
                        shard,
                        sender: tx.sender,
                        inputs: tx.inputs.clone(),
                        expires_at: tx.expires_at,
                        rotations,
                        posted_at: ctx.pulse,
                        deadline,
                        missed: 0,
                        signer,
                    },
                );
                protocol
                    .schedule_event(deadline, ScheduledEvent::ForcedDeadline { intent: digest });
                outcome.events.push(Event::ForcedPosted {
                    intent: digest,
                    shard,
                    deadline,
                });
            }
        }
    }

    // Scheduled events due now (and any left from earlier Pulses). Every due
    // entry is taken out first; whatever processing schedules waits for a
    // later Pulse.
    let due: Vec<u64> = protocol
        .schedule
        .1
        .due
        .range(..=ctx.pulse.0)
        .map(|(p, _)| *p)
        .collect();
    let mut taken = Vec::with_capacity(due.len());
    for at in due {
        taken.push(protocol.schedule.1.due.remove(&at).unwrap_or_default());
        protocol.schedule.2 = true;
    }
    for events in taken {
        for event in events {
            match event {
                ScheduledEvent::SealBatch { market, batch } => {
                    let Some(state) = protocol.market(&mut run, &market)? else {
                        continue;
                    };
                    let deadline = state.clear_deadline;
                    let Some(entry) = state.batches.get_mut(&batch) else {
                        continue;
                    };
                    if entry.status != BatchStatus::Open {
                        continue;
                    }
                    entry.status = BatchStatus::Sealed {
                        beacon: ctx.beacon,
                        at: ctx.pulse,
                    };
                    let orders = entry.orders.len() as u64;
                    outcome.events.push(Event::BatchSealed {
                        market,
                        batch,
                        orders,
                    });
                    protocol.schedule_event(
                        Pulse(ctx.pulse.0.saturating_add(deadline)),
                        ScheduledEvent::BatchDeadline { market, batch },
                    );
                }
                ScheduledEvent::BatchDeadline { market, batch } => {
                    let Some(state) = protocol.market(&mut run, &market)? else {
                        continue;
                    };
                    let Some(entry) = state.batches.get_mut(&batch) else {
                        continue;
                    };
                    if !matches!(entry.status, BatchStatus::Sealed { .. }) {
                        continue;
                    }
                    entry.status = BatchStatus::Refunded;
                    let refunds: Vec<(AccountId, AssetId, Amount)> = entry
                        .orders
                        .iter()
                        .map(|o| (o.owner, o.escrow_asset, o.escrow))
                        .collect();
                    for (owner, asset, amount) in refunds {
                        run.pay(owner, asset, amount);
                    }
                    outcome.events.push(Event::BatchRefunded { market, batch });
                }
                ScheduledEvent::ForcedDeadline { intent } => {
                    let Some(obligation) = protocol.forced(&mut run)?.pending.get(&intent).cloned()
                    else {
                        continue;
                    };
                    if !still_executable(&mut run, &obligation)? {
                        protocol.forced(&mut run)?.pending.remove(&intent);
                        outcome.events.push(Event::ForcedResolved { intent });
                        continue;
                    }
                    // The intent could still execute: its shard censored it.
                    outcome.events.push(Event::ForcedOverdue {
                        intent,
                        shard: obligation.shard,
                    });
                    let window = protocol.forced(&mut run)?.params.window.max(1);
                    let set = protocol.validators(&mut run)?;
                    if let Some(rotation) = rotate(set, ctx, obligation.shard, window)? {
                        outcome.events.push(Event::ShardRotated {
                            shard: obligation.shard,
                            rotation,
                        });
                    }
                    let obligations = protocol.forced(&mut run)?;
                    let window = obligations.params.window.max(1);
                    if ctx.pulse >= obligation.expires_at {
                        // It can never execute now; the violation stands recorded.
                        obligations.pending.remove(&intent);
                    } else {
                        let deadline = Pulse(
                            ctx.pulse
                                .0
                                .saturating_add(window)
                                .min(obligation.expires_at.0),
                        );
                        if let Some(entry) = obligations.pending.get_mut(&intent) {
                            entry.missed = entry.missed.saturating_add(1);
                            entry.deadline = deadline;
                        }
                        protocol
                            .schedule_event(deadline, ScheduledEvent::ForcedDeadline { intent });
                    }
                }
                ScheduledEvent::EpochEnd => {
                    if ctx.pulse != protocol.validators(&mut run)?.epoch_end() {
                        continue;
                    }
                    let treasury = protocol.treasury(&mut run)?;
                    let collected = treasury
                        .fees
                        .get(&AssetId::usd())
                        .copied()
                        .unwrap_or(Amount::ZERO);
                    let (set, book) = protocol.staking(&mut run)?;
                    let paid = end_epoch(set, book, ctx, collected)?;
                    let (epoch, active, next) = (set.epoch, set.active.len(), set.epoch_end());
                    let treasury = protocol.treasury(&mut run)?;
                    let entry = treasury
                        .balances
                        .entry(AssetId::usd())
                        .or_insert(Amount::ZERO);
                    *entry = entry.checked_sub(paid)?;
                    protocol.schedule_event(next, ScheduledEvent::EpochEnd);
                    outcome.events.push(Event::EpochChanged {
                        epoch,
                        active: active as u32,
                        rewards: paid,
                    });
                }
                ScheduledEvent::RegistryTick => {
                    let domain = protocol.assurance(&mut run)?;
                    // Advance to the next Pulse: claims valid through this one
                    // expire now, reservations ending now are released, and
                    // pool funds no longer needed for any liability are freed.
                    let expiries = domain
                        .registry
                        .advance_to(Pulse(ctx.pulse.0.saturating_add(1)));
                    let released = Amount::sum(expiries.released_pools.values())?;
                    if released > Amount::ZERO {
                        domain.vault = domain.vault.checked_sub(released)?;
                        let treasury = protocol.treasury(&mut run)?;
                        let entry = treasury
                            .balances
                            .entry(AssetId::usd())
                            .or_insert(Amount::ZERO);
                        *entry = entry.saturating_add(released);
                    }
                }
            }
        }
    }

    protocol.write_back(&mut run);
    record_beacon(&mut run, ctx)?;
    Ok(outcome)
}

/// Records the beacon of a Pulse that is a multiple of
/// [`crate::beacons::BEACON_INTERVAL`] in its slot (spec §6.3), last in the
/// deferred phase.
fn record_beacon<S: ObjectStore>(
    run: &mut DeferredRun<'_, '_, S>,
    ctx: &PulseContext,
) -> Result<(), ExecError> {
    let Some(slot) = crate::beacons::slot_of(ctx.pulse) else {
        return Ok(());
    };
    let id = well_known::beacon(slot);
    let data = ObjectData::Beacon(crate::beacons::BeaconRecord {
        pulse: ctx.pulse,
        beacon: ctx.beacon,
    });
    let object = match run.load(&id)? {
        Some(mut object) => {
            if !matches!(object.data, ObjectData::Beacon(_)) {
                return Err(ExecError::Invalid("beacon record corrupted".into()));
            }
            object.data = data;
            object
        }
        None => Object {
            id,
            version: Version(0),
            owner: Owner::Shared(well_known::beacons_domain()),
            data,
        },
    };
    run.store(object);
    Ok(())
}

/// Whether a forced intent may become an obligation: everything `prepare`
/// checks (§4.1 of the specification), read-only, plus that it is not a
/// faucet transaction and does not expire in the Pulse it is posted in.
/// Returns its signer and the signer's policy rotations, or why not.
fn admissible<S: ObjectStore>(
    run: &mut DeferredRun<'_, '_, S>,
    ctx: &PulseContext,
    signed: &SignedTransaction,
) -> Result<Result<(AccountId, u32), String>, ExecError> {
    let tx = &signed.tx;
    if matches!(tx.kind, TxKind::Faucet { .. }) {
        return Ok(Err("faucet transactions cannot be forced".into()));
    }
    if ctx.pulse >= tx.expires_at {
        return Ok(Err("it expires before its shard could include it".into()));
    }
    if tx.inputs.is_empty() || tx.inputs.len() > MAX_INPUTS {
        return Ok(Err(format!("it needs 1 to {MAX_INPUTS} inputs")));
    }
    let unique: BTreeSet<ObjectId> = tx.inputs.iter().map(|r| r.id).collect();
    if unique.len() != tx.inputs.len() {
        return Ok(Err("duplicate inputs".into()));
    }
    match required_fee(&ctx.config, signed) {
        Some(floor) if tx.fee >= floor => {}
        _ => return Ok(Err("fee below the minimum".into())),
    }
    // The authorizing policy (spec §4.3), read with try_load and storing
    // nothing: the signer is whose policy it is.
    let authority = match find_authority(signed, &mut |id| run.load(id))? {
        Ok(authority) => authority,
        Err(reason) => return Ok(Err(reason)),
    };
    // Always checked, even when the node skips signature checks for
    // transactions it verified when locking their inputs: forced intents
    // were never locked.
    if check_authorization(&ctx.config, signed, &authority.policy).is_err() {
        return Ok(Err("its authorization does not verify".into()));
    }
    for (index, input) in tx.inputs.iter().enumerate() {
        let Some(object) = run.load(&input.id)? else {
            return Ok(Err(format!("input {} does not exist", input.id)));
        };
        if object.version != input.version || object.is_tombstone() {
            return Ok(Err(format!(
                "input {} was spent or moved (executed or invalid)",
                input.id
            )));
        }
        if object.owner != Owner::Account(tx.sender) {
            return Ok(Err(format!("input {} is not the sender's", input.id)));
        }
        if index == 0 {
            match object.as_coin() {
                Some(coin) if coin.asset == AssetId::usd() && coin.amount >= tx.fee => {}
                _ => return Ok(Err("inputs[0] must be a USD coin covering the fee".into())),
            }
        }
    }
    Ok(Ok((authority.signer, authority.rotations)))
}

/// Whether an obligation's intent could still execute: every input is at the
/// version it spends, unspent and the sender's, and the signer's policy has
/// not rotated.
fn still_executable<S: ObjectStore>(
    run: &mut DeferredRun<'_, '_, S>,
    obligation: &Obligation,
) -> Result<bool, ExecError> {
    for input in &obligation.inputs {
        match run.load(&input.id)? {
            Some(object)
                if object.version == input.version
                    && !object.is_tombstone()
                    && object.owner == Owner::Account(obligation.sender) => {}
            _ => return Ok(false),
        }
    }
    let rotations = match run.load(&well_known::account(&obligation.signer))? {
        Some(Object {
            data: ObjectData::Account(state),
            ..
        }) => state.rotations,
        Some(_) => return Ok(false),
        None => 0,
    };
    Ok(rotations == obligation.rotations)
}

/// Freezes `shard`'s committee out and samples its replacement, unless the
/// chain has halted (no active set to sample from) or the shard already
/// rotated in the last `window` Pulses (the new committee gets a full window
/// before it can be judged). Returns the shard's rotation count if it
/// rotated.
fn rotate(
    set: &mut ValidatorSet,
    ctx: &PulseContext,
    shard: u16,
    window: u64,
) -> Result<Option<u32>, ExecError> {
    if set.active.is_empty() {
        return Ok(None);
    }
    if let Some(last) = set.rotations.get(&shard) {
        if ctx.pulse.0 < last.at.0.saturating_add(window) {
            return Ok(None);
        }
    }
    let invalid = |e: sion_consensus::ConsensusError| ExecError::Invalid(e.to_string());
    let previous = set.lane_members(ShardId(shard)).map_err(invalid)?;
    let count = set
        .rotations
        .get(&shard)
        .map(|r| r.count)
        .unwrap_or(0)
        .saturating_add(1);
    let committee = resample_committee(
        &set.active,
        usize::from(set.params.seats),
        &rotation_seed(&ctx.beacon, ShardId(shard), count),
        Epoch(set.epoch),
        ShardId(shard),
        &previous,
    )
    .map_err(invalid)?;
    set.rotations.insert(
        shard,
        ShardRotation {
            count,
            members: committee.members().iter().map(|(v, _)| *v).collect(),
            at: ctx.pulse,
        },
    );
    Ok(Some(count))
}

/// Ends the current epoch at the end of `ctx.pulse`, given the treasury's
/// running total of USD fees: pays the epoch's validators their share of
/// the fees collected during the epoch (nothing else the treasury received),
/// admits and retires validators, picks the next active set (empty if no
/// validator is eligible: the chain then halts) and starts the next epoch.
/// Returns the rewards paid (to be taken from the treasury).
fn end_epoch(
    set: &mut ValidatorSet,
    mut book: Option<&mut DelegationBook>,
    ctx: &PulseContext,
    collected: Amount,
) -> Result<Amount, ExecError> {
    let overflow = || ExecError::Core(sion_core::CoreError::Overflow("pool rewards"));
    let gain = collected.saturating_sub(set.fees_mark);
    let pool = crate::validators::permille_of(gain, set.params.reward_permille);
    let total: u64 = set.active.iter().map(|(_, w)| *w).sum();
    let mut paid = Amount::ZERO;
    for (validator, w) in set.active.clone() {
        let Some(record) = set.validators.get_mut(&validator) else {
            continue;
        };
        if !matches!(
            record.status,
            ValidatorStatus::Active | ValidatorStatus::Leaving
        ) {
            continue;
        }
        let share = share_of(pool, w, total);
        // The pool's earning stake takes its proportional part, less the
        // operator's commission, credited per earning share.
        let mut operator = share;
        if let Some(delegated) = book
            .as_deref_mut()
            .and_then(|b| b.pools.get_mut(&validator))
        {
            let earning = delegated.earning_value().ok_or_else(overflow)?;
            if delegated.earning > 0 && earning > Amount::ZERO {
                let part = mul_div(share.0, earning.0, record.stake.checked_add(earning)?.0)
                    .ok_or_else(overflow)?;
                let commission =
                    crate::validators::permille_of(Amount(part), delegated.commission_permille);
                let delegators = Amount(part).checked_sub(commission)?;
                let credit =
                    mul_div(delegators.0, REWARD_SCALE, delegated.earning).ok_or_else(overflow)?;
                delegated.reward_per_share = delegated
                    .reward_per_share
                    .checked_add(credit)
                    .ok_or_else(overflow)?;
                delegated.rewards_held = delegated.rewards_held.checked_add(delegators)?;
                operator = share.checked_sub(delegators)?;
            }
        }
        record.rewards = record.rewards.checked_add(operator)?;
        paid = paid.checked_add(share)?;
    }
    // Delegations made this epoch join, from the accumulator after its
    // rewards; commission changes take effect.
    if let Some(book) = book.as_deref_mut() {
        for delegated in book.pools.values_mut() {
            if delegated.pending_delegations > 0 {
                delegated.joins.insert(
                    set.epoch,
                    Join {
                        reward_per_share: delegated.reward_per_share,
                        waiting: delegated.pending_delegations,
                    },
                );
            }
            delegated.earning = delegated
                .earning
                .checked_add(delegated.pending)
                .ok_or_else(overflow)?;
            delegated.pending = 0;
            delegated.pending_delegations = 0;
            if let Some(next) = delegated.next_commission.take() {
                delegated.commission_permille = next;
            }
        }
    }
    let unbonded = Pulse(ctx.pulse.0.saturating_add(set.params.unbonding));
    for record in set.validators.values_mut() {
        record.status = match record.status {
            ValidatorStatus::Joining => ValidatorStatus::Active,
            ValidatorStatus::Leaving => ValidatorStatus::Exited {
                withdrawable_at: unbonded,
            },
            other => other,
        };
    }
    // Validators rank by their own stake plus their pool's earning stake.
    let mut candidates: Vec<(Amount, sion_core::ids::ValidatorId)> = Vec::new();
    for record in set
        .validators
        .values()
        .filter(|r| r.status == ValidatorStatus::Active)
    {
        let pooled = match book.as_deref().and_then(|b| b.pools.get(&record.id)) {
            Some(delegated) => delegated.earning_value().ok_or_else(overflow)?,
            None => Amount::ZERO,
        };
        candidates.push((record.stake.checked_add(pooled)?, record.id));
    }
    candidates.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    candidates.truncate(usize::from(set.params.max_active.max(1)));
    let mut active: Vec<(sion_core::ids::ValidatorId, u64)> = candidates
        .iter()
        .map(|(stake, id)| (*id, weight(*stake)))
        .collect();
    active.sort();
    // Slashed validators lose their seats here even if nobody else is
    // eligible: a chain whose every eligible validator was slashed halts (no
    // committee certifies its next Pulse) rather than run on proven
    // equivocators with nothing at stake.
    set.active = active;
    set.epoch = set.epoch.saturating_add(1);
    set.epoch_start = ctx.pulse.next();
    set.seed = ctx.beacon;
    set.rotations.clear();
    set.fees_mark = collected;
    Ok(paid)
}

/// Issues a claim into the registry; returns the last Pulse of its window.
///
/// A claim may rest directly only on its backer's bonded atoms: a bond is a
/// promise about its owner's own statements, so nobody can put someone
/// else's bond at stake (other parties' attestations enter as references to
/// claims they issued). References must point to claims active since an
/// earlier Pulse.
fn issue(
    registry: &mut Registry,
    pulse: Pulse,
    issuer: AccountId,
    backer: AccountId,
    id: ClaimId,
    body: &ClaimBody,
) -> Result<Pulse, String> {
    for atom in body.formula.direct_atoms() {
        let Some(info) = registry.atom(&atom) else {
            return Err(format!("unknown atom {atom}"));
        };
        if info.bonded && bond_owner(info) != Some(backer) {
            return Err(format!(
                "the claim rests on bonded atom {} that is not the issuer's",
                info.label
            ));
        }
    }
    for referenced in body.formula.claim_refs() {
        match registry.claim(&referenced) {
            Some(record) if record.issued_at < pulse && record.status == ClaimStatus::Active => {}
            Some(_) => {
                return Err(format!(
                    "referenced claim {referenced} is not an active claim from an earlier Pulse"
                ))
            }
            None => return Err(format!("unknown claim {referenced}")),
        }
    }
    let valid_until = Pulse(pulse.0.saturating_add(body.valid_for));
    let record = ClaimRecord {
        id,
        issuer,
        formula: body.formula.clone(),
        adjudicator: body.adjudicator,
        detection: Ratio::permille(u64::from(body.detection_permille)),
        issued_at: pulse,
        valid_until,
        challenge_until: Pulse(valid_until.0.saturating_add(body.challenge_period)),
        status: ClaimStatus::Active,
        sequence: 0,
    };
    registry.issue_claim(record).map_err(|e| e.to_string())?;
    Ok(valid_until)
}
