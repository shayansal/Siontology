//! Transactions and their coordination classes.
//!
//! The coordination class of a transaction is derived from what it does,
//! never declared: knowledge-only effects need no ordering, owned inputs need
//! a lane certificate, and operations on shared objects need consensus in
//! that object's contention domain. Every transaction has at least one owned
//! input (its gas coin), which also gives replay protection: each object
//! version can be consumed by one transaction only.

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sion_assurance::{AtomClass, Policy};
use sion_clearing::ClearingResult;
use sion_core::encode::encode;
use sion_core::hash::Hash32;
use sion_core::ids::{
    AccountId, AssetId, AtomId, ClaimId, DomainId, ObjectId, Pulse, TxDigest, ValidatorId,
};
use sion_core::Amount;
use sion_crypto::{
    AuthPolicy, Authorization, CryptoError, DualKeypair, KemPublicKey, KeyShare, PublicKey,
};

use crate::charter::{Caveats, Narrowing, Spawn};
use crate::claim::ClaimBody;
use crate::cohort::Materialize;
use crate::market::Submission;
use crate::object::{well_known, CoordinationClass, ObjectRef};
use crate::state::ChainConfig;
use crate::storage::CustodyProof;
use crate::task::Acceptance;
use crate::validators::Equivocation;

/// The signing domain for transactions.
pub const TX_DOMAIN: &str = "sion/tx/v1";

/// The signing context of transactions on the chain with this
/// configuration: [`TX_DOMAIN`], a slash, and the configuration digest in
/// lowercase hex. A transaction signed for one chain does not verify on any
/// chain with another configuration (chain identifiers must differ).
pub fn signing_context(config: &ChainConfig) -> String {
    format!("{TX_DOMAIN}/{}", config.digest().to_hex())
}

/// The signing context of batches on the chain with this configuration
/// (spec §4.3): [`TX_DOMAIN`], `/batch/`, and the configuration digest in
/// lowercase hex. It differs from [`signing_context`], so a transaction's
/// signature never verifies as a batch's, nor a batch's as a transaction's.
pub fn batch_signing_context(config: &ChainConfig) -> String {
    format!("{TX_DOMAIN}/batch/{}", config.digest().to_hex())
}

/// The most owned inputs a transaction may name.
pub const MAX_INPUTS: usize = 256;

/// The most transactions one batch may authorize (spec §4.3).
pub const MAX_BATCH: u64 = 256;

/// The part of the base fee, in thousandths, that pays for checking an
/// authorization (spec §4.3). A batch's members split it, so a member of a
/// batch of 256 pays a 256th of it.
pub const SIGNATURE_SHARE_PERMILLE: u128 = 800;

/// What one authorization of a batch signs (spec §4.3): who signs, how many
/// transactions the batch holds, and the RFC 9162 Merkle root over their
/// leaves ([`batch_leaf`]), in the batch's order.
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
pub struct TxBatch {
    /// The account whose policy the authorization satisfies.
    pub signer: AccountId,
    /// How many transactions the batch holds, 1 to [`MAX_BATCH`].
    pub size: u64,
    /// The Merkle root over the members' leaves.
    pub root: Hash32,
}

impl TxBatch {
    /// The message the batch's authorization signs, in
    /// [`batch_signing_context`]: `H("sion/tx/batch", enc(batch))`.
    pub fn message(&self) -> Hash32 {
        sion_core::hash::hash_domain("sion/tx/batch", &encode(self))
    }
}

/// The Merkle leaf of a batch member with this digest:
/// `merkle::leaf_hash(digest bytes)`.
pub fn batch_leaf(digest: &TxDigest) -> Hash32 {
    sion_core::merkle::leaf_hash(&digest.0 .0)
}

/// A batch member's place in its batch (spec §4.3).
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct BatchMember {
    /// The batch the authorization signs.
    pub batch: TxBatch,
    /// The member's position in the batch.
    pub index: u64,
    /// Its inclusion path to the batch's root.
    pub path: Vec<Hash32>,
    /// For a keyless sender, its charter: the subagent's id
    /// ([`crate::charter::keyless_account`]).
    pub charter: Option<ObjectId>,
}

/// A whole batch as a client hands it to a node: the batch, its one
/// authorization, and each member's body with, for a keyless sender, its
/// charter. Not consensus data: a node checks that the members give the
/// batch's root and size, then builds each member's path
/// ([`BatchEnvelope::members`]), so the signatures cross the wire once.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct BatchEnvelope {
    /// The batch.
    pub batch: TxBatch,
    /// Its authorization.
    pub auth: Authorization,
    /// The members, in order.
    pub members: Vec<(Transaction, Option<ObjectId>)>,
}

impl BatchEnvelope {
    /// The envelope of a whole batch, from all its members in order (as
    /// [`sign_batch`] returns them); `None` if they are not one whole batch.
    pub fn of(members: &[SignedTransaction]) -> Option<BatchEnvelope> {
        let first = members.first()?;
        let batch = first.batch.as_ref()?.batch;
        if batch.size != members.len() as u64 {
            return None;
        }
        let mut bodies = Vec::with_capacity(members.len());
        for (index, member) in members.iter().enumerate() {
            let place = member.batch.as_ref()?;
            if place.batch != batch || place.index != index as u64 || member.auth != first.auth {
                return None;
            }
            bodies.push((member.tx.clone(), place.charter));
        }
        Some(BatchEnvelope {
            batch,
            auth: first.auth.clone(),
            members: bodies,
        })
    }

    /// The members as signed transactions, each with its path, if they are
    /// the whole batch: 1 to [`MAX_BATCH`] of them, as many as the batch
    /// says, and their leaves give its root. The authorization is not
    /// checked here.
    pub fn members(self) -> Result<Vec<SignedTransaction>, String> {
        let size = self.members.len() as u64;
        if self.members.is_empty() || size > MAX_BATCH {
            return Err(format!(
                "a batch holds 1 to {MAX_BATCH} transactions, not {size}"
            ));
        }
        if size != self.batch.size {
            return Err(format!(
                "the batch says it holds {}, the envelope has {size}",
                self.batch.size
            ));
        }
        let leaves: Vec<Hash32> = self
            .members
            .iter()
            .map(|(tx, _)| batch_leaf(&tx.digest()))
            .collect();
        if sion_core::merkle::root(&leaves) != self.batch.root {
            return Err("the members do not give the batch's root".into());
        }
        self.members
            .into_iter()
            .enumerate()
            .map(|(index, (tx, charter))| {
                let path = sion_core::merkle::inclusion_proof(&leaves, index)
                    .map_err(|e| e.to_string())?;
                Ok(SignedTransaction {
                    tx,
                    auth: self.auth.clone(),
                    batch: Some(BatchMember {
                        batch: self.batch,
                        index: index as u64,
                        path,
                        charter,
                    }),
                })
            })
            .collect()
    }
}

/// The fee floor of a transaction body of `size` bytes (spec §4.3): the
/// configuration's [`ChainConfig::min_fee`] for a transaction signed by
/// itself (`batch_size = None`); for a member of a batch of `batch_size`,
/// that less [`SIGNATURE_SHARE_PERMILLE`] of the base fee, plus the
/// member's part of it, rounded up. `None` only if the share overflows.
pub fn fee_floor(config: &ChainConfig, size: usize, batch_size: Option<u64>) -> Option<Amount> {
    let floor = config.min_fee(size);
    let Some(members) = batch_size else {
        return Some(floor);
    };
    let share = config.base_fee.0.checked_mul(SIGNATURE_SHARE_PERMILLE)? / 1000;
    let part = share.div_ceil(u128::from(members.max(1)));
    Some(Amount(floor.0 - share + part))
}

/// A signed transaction's fee floor ([`fee_floor`] of its body and batch).
pub fn required_fee(config: &ChainConfig, signed: &SignedTransaction) -> Option<Amount> {
    fee_floor(
        config,
        signed.tx.size(),
        signed.batch.as_ref().map(|m| m.batch.size),
    )
}

/// Signs `members` as one batch with `keys`, the keys of `signer`
/// (spec §4.3): computes the root over their digests, signs the batch once
/// and gives each member the authorization, its position and its path.
/// Each member is a body and, for a keyless sender, its charter. Fees are
/// the caller's: a member pays at least [`fee_floor`] for this batch's size.
pub fn sign_batch(
    keys: &DualKeypair,
    signer: AccountId,
    config: &ChainConfig,
    members: Vec<(Transaction, Option<ObjectId>)>,
) -> Result<Vec<SignedTransaction>, crate::error::ExecError> {
    let size = members.len() as u64;
    if members.is_empty() || size > MAX_BATCH {
        return Err(crate::error::ExecError::Invalid(format!(
            "a batch holds 1 to {MAX_BATCH} transactions, not {size}"
        )));
    }
    let leaves: Vec<Hash32> = members
        .iter()
        .map(|(tx, _)| batch_leaf(&tx.digest()))
        .collect();
    let batch = TxBatch {
        signer,
        size,
        root: sion_core::merkle::root(&leaves),
    };
    let auth = keys.authorize(&batch_signing_context(config), &batch.message().0)?;
    members
        .into_iter()
        .enumerate()
        .map(|(index, (tx, charter))| {
            let path = sion_core::merkle::inclusion_proof(&leaves, index)?;
            Ok(SignedTransaction {
                tx,
                auth: auth.clone(),
                batch: Some(BatchMember {
                    batch,
                    index: index as u64,
                    path,
                    charter,
                }),
            })
        })
        .collect()
}

/// The largest amount one faucet grant may mint. Bounding grants keeps every
/// supply total far from overflow.
pub const MAX_GRANT: Amount = Amount::units(1_000_000_000);

/// What a transaction does.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub enum TxKind {
    /// Pays outputs from the sender's coins of `asset`; change returns to the sender.
    Transfer {
        /// Asset transferred.
        asset: AssetId,
        /// Recipients and amounts.
        outputs: Vec<(AccountId, Amount)>,
    },
    /// Devnet faucet: mints each grant. Only the configured faucet account
    /// may send it; its input is the faucet's account object.
    Faucet {
        /// Recipients, assets and amounts (at most 256).
        grants: Vec<(AccountId, AssetId, Amount)>,
    },
    /// Rotates the sender's authorization policy (the account ID is unchanged).
    RotatePolicy {
        /// New policy.
        policy: AuthPolicy,
        /// New viewing key.
        viewing_key: Option<KemPublicKey>,
    },
    /// Issues a claim.
    IssueClaim {
        /// The claim.
        claim: ClaimBody,
    },
    /// Registers an unbonded atom (for example a circuit). Attack costs are
    /// governance-set at genesis and cannot be registered this way.
    RegisterAtom {
        /// Class.
        class: AtomClass,
        /// Label.
        label: String,
    },
    /// Posts (or tops up) a bond behind the sender's bonded atom
    /// `bond:<sender>:<label>`.
    PostBond {
        /// Atom class (economic, institutional or adjudicator).
        class: AtomClass,
        /// Label within the sender's namespace.
        label: String,
        /// Amount in USD.
        amount: Amount,
    },
    /// Accepts a claim under a policy and reserves κ·V of its backing.
    Accept {
        /// The claim.
        claim: ClaimId,
        /// Value at risk.
        value_at_risk: Amount,
        /// The consumer's policy.
        policy: Policy,
        /// End of reliance.
        until: Pulse,
        /// Distinguishes repeated acceptances.
        nonce: u64,
    },
    /// Releases a reservation.
    Release {
        /// The reservation.
        reservation: Hash32,
    },
    /// Challenges a bonded atom of a claim.
    Challenge {
        /// The claim.
        claim: ClaimId,
        /// The challenged atom.
        target: AtomId,
        /// Digest of the counter-evidence.
        evidence: Hash32,
        /// Deposit in USD.
        deposit: Amount,
    },
    /// The claim's adjudicator rules on a challenge.
    Rule {
        /// The challenge.
        challenge: Hash32,
        /// Whether the atom is broken.
        upheld: bool,
    },
    /// Withdraws the free part of a bond after its lock.
    WithdrawBond {
        /// The bonded atom.
        atom: AtomId,
    },
    /// Creates an Agent Charter and funds the agent's allotment.
    CreateCharter {
        /// The agent.
        agent: AccountId,
        /// The caveats.
        caveats: Caveats,
        /// Budget in USD.
        funding: Amount,
        /// Bond behind the agent's receipts, in USD.
        agent_bond: Amount,
    },
    /// The agent delegates part of its allotment under narrower caveats.
    Delegate {
        /// The parent charter.
        charter: ObjectId,
        /// The sub-agent.
        sub_agent: AccountId,
        /// Narrower caveats.
        caveats: Caveats,
        /// Amount moved from the agent's allotment.
        amount: Amount,
    },
    /// The agent pays from its allotment.
    AgentPay {
        /// The charter.
        charter: ObjectId,
        /// Payee.
        payee: AccountId,
        /// Amount.
        amount: Amount,
        /// Purpose (recorded in the receipt).
        purpose: String,
        /// Evidence claim for large payments.
        evidence: Option<ClaimId>,
    },
    /// The principal revokes a charter.
    RevokeCharter {
        /// The charter.
        charter: ObjectId,
    },
    /// After a charter is revoked or expired (or an ancestor is revoked),
    /// its principal or root principal returns what is left in its
    /// allotments to the root principal, whose funds they were.
    ReclaimCharter {
        /// The charter.
        charter: ObjectId,
    },
    /// Creates a bounded total split into allotments.
    CreateBounded {
        /// Label.
        label: String,
        /// Asset escrowed.
        asset: AssetId,
        /// Total.
        total: Amount,
        /// Initial allotments.
        allotments: Vec<(AccountId, Amount)>,
    },
    /// The writer spends from its allotment.
    SpendAllotment {
        /// Recipient.
        to: AccountId,
        /// Amount.
        amount: Amount,
    },
    /// The writer returns the rest of its allotment to the bounded total.
    ReturnAllotment,
    /// The manager issues a new allotment from the unallocated amount.
    Rebalance {
        /// The bounded total.
        bounded: ObjectId,
        /// The new allotment's single writer.
        holder: AccountId,
        /// Amount.
        amount: Amount,
    },
    /// Creates a market.
    CreateMarket {
        /// Name.
        label: String,
        /// The good.
        good: AssetId,
        /// The numeraire.
        numeraire: AssetId,
        /// Pulses per batch.
        batch_length: u64,
        /// Pulses allowed for clearing after sealing.
        clear_deadline: u64,
        /// Committee for sealed orders.
        committee: Vec<KemPublicKey>,
        /// Reveal threshold.
        threshold: u8,
    },
    /// Submits an order (escrow is drawn from the sender's coins).
    SubmitOrder {
        /// The market.
        market: ObjectId,
        /// The order.
        submission: Submission,
    },
    /// Reveals sealed orders of a sealed batch with committee key shares.
    RevealBatch {
        /// The market.
        market: ObjectId,
        /// Batch index.
        batch: u64,
        /// Shares per sealed order.
        shares: Vec<(Hash32, Vec<KeyShare>)>,
    },
    /// A solver submits a certified clearing for a sealed batch.
    ClearBatch {
        /// The market.
        market: ObjectId,
        /// Batch index.
        batch: u64,
        /// The clearing with its optimality certificate.
        result: ClearingResult,
    },
    /// Stakes USD from the sender's coins to register a validator, which
    /// joins the active set at the next epoch boundary. `possession` is the
    /// validator's signature over
    /// [`possession_digest`](crate::validators::possession_digest).
    RegisterValidator {
        /// The validator's ML-DSA-44 public key.
        lattice: PublicKey,
        /// The validator's SLH-DSA public key.
        hash: PublicKey,
        /// The tip of its beacon hash chain.
        beacon_tip: Hash32,
        /// Stake in USD.
        stake: Amount,
        /// Proof that the validator holds both keys.
        possession: Vec<u8>,
    },
    /// The operator takes its validator out at the next epoch boundary.
    Unstake {
        /// The validator.
        validator: ValidatorId,
    },
    /// The operator withdraws stake and unclaimed rewards after unbonding.
    WithdrawStake {
        /// The validator.
        validator: ValidatorId,
    },
    /// The operator collects its validator's rewards.
    ClaimRewards {
        /// The validator.
        validator: ValidatorId,
    },
    /// Reports a validator's equivocation: its stake is slashed, a share
    /// to the sender and the rest to the treasury.
    ReportEquivocation {
        /// The signed evidence.
        evidence: Box<Equivocation>,
    },
    /// Escrows `deposit` from the sender's coins for `payee`, who is then
    /// paid off the ledger, `unit` per step, by the sender revealing the
    /// elements of a hash chain ending at `anchor` (see [`crate::stream`]).
    OpenStream {
        /// Who is paid.
        payee: AccountId,
        /// The asset escrowed and paid.
        asset: AssetId,
        /// The value of one step.
        unit: Amount,
        /// Steps in the chain.
        length: u64,
        /// The chain's last element, `w_0`.
        anchor: Hash32,
        /// The escrow.
        deposit: Amount,
        /// The last Pulse in which the payee may settle.
        until: Pulse,
        /// A commitment to every `spacing`-th chain element, which lets the
        /// payee settle any number of steps for a bounded number of hashes.
        checkpoints: Option<crate::stream::Checkpoints>,
    },
    /// The payee collects the steps up to `position`, proven by the chain
    /// element `value` at that position: by walking down to the head, or by
    /// a `jump` to a checkpoint.
    SettleStream {
        /// The stream.
        stream: ObjectId,
        /// The position settled to.
        position: u64,
        /// The chain element at `position`.
        value: Hash32,
        /// The checkpoint below `position` and its Merkle path.
        jump: Option<crate::stream::Jump>,
    },
    /// The payer adds to the escrow and may extend the stream's last Pulse.
    TopUpStream {
        /// The stream.
        stream: ObjectId,
        /// Added to the escrow (may be zero if `until` is extended).
        deposit: Amount,
        /// The new last Pulse (never earlier than the current one).
        until: Pulse,
    },
    /// Closes a stream and returns what is left in escrow to the payer: by
    /// the payee at any time, or by the payer once the stream has expired.
    CloseStream {
        /// The stream.
        stream: ObjectId,
    },
    /// Registers an agent identity: a charter with an identity record
    /// (spec §5.35).
    RegisterAgent {
        /// The agent's own account.
        agent: AccountId,
        /// Commitment to the agent's card.
        card: Hash32,
        /// The caveats.
        caveats: Caveats,
        /// Budget in USD.
        funding: Amount,
        /// Bond behind the agent's receipts, in USD.
        agent_bond: Amount,
    },
    /// An agent launches subagents, each with its own identity, account,
    /// narrower caveats and budget (spec §5.36).
    SpawnAgents {
        /// The parent's charter.
        charter: ObjectId,
        /// The subagents, at most `MAX_SPAWN_BATCH`.
        children: Vec<Spawn>,
    },
    /// An agent replaces its card commitment (spec §5.37).
    SetAgentCard {
        /// The agent's charter.
        charter: ObjectId,
        /// The new commitment.
        card: Hash32,
    },
    /// An overseer suspends a charter and everything below it (spec §5.38).
    SuspendAgent {
        /// The charter.
        charter: ObjectId,
    },
    /// The suspender or the backer lifts a suspension (spec §5.39).
    ResumeAgent {
        /// The charter.
        charter: ObjectId,
    },
    /// Opens a memory log that only the sender writes (spec §5.40).
    OpenMemory {
        /// The agent identity the log belongs to, if any; the sender must
        /// be its agent.
        agent: Option<ObjectId>,
        /// A commitment to the log's policy, kept off the ledger.
        policy: Hash32,
    },
    /// Moves a memory log's head forward (spec §5.41). The log is
    /// `inputs[1]`.
    CommitMemory {
        /// The log's new size, in atoms.
        size: u64,
        /// The Merkle tree hash over the first `size` atoms' leaves.
        root: Hash32,
        /// The root of the log's index.
        index: Hash32,
        /// The root of the log's storage manifest.
        manifest: Hash32,
        /// A consistency proof from the current head to the new one.
        proof: Vec<Hash32>,
        /// Whether to seal the log after this commit.
        seal: bool,
    },
    /// Escrows `amount` for a task whose acceptance is fixed now (spec
    /// §5.42). With `charter`, the escrow comes from that agent's
    /// allotment (`inputs[1]`), under its caveats; otherwise from coins.
    PostTask {
        /// A commitment to the task's specification.
        spec: Hash32,
        /// The rule a result must pass.
        acceptance: Acceptance,
        /// The worker, or `None` for an open task.
        worker: Option<AccountId>,
        /// The escrow.
        amount: Amount,
        /// The last Pulse a result may be settled.
        deadline: Pulse,
        /// The agent's charter whose allotment pays, if any.
        charter: Option<ObjectId>,
    },
    /// A worker settles a task with its result claim and the attestations
    /// it rests on (spec §5.43).
    SettleTask {
        /// The task.
        task: ObjectId,
        /// The body of the sender's result claim.
        result: ClaimBody,
        /// Verifiers' attestations: each verifier and its claim's body.
        attestations: Vec<(AccountId, ClaimBody)>,
    },
    /// Cancels an open task and refunds its escrow (spec §5.44).
    CancelTask {
        /// The task.
        task: ObjectId,
    },
    /// Delegates stake to a validator's pool without handing it over (spec
    /// §5.45): the sender keeps a delegation object holding pool shares.
    DelegateStake {
        /// The validator.
        validator: ValidatorId,
        /// Stake, in the stake asset.
        amount: Amount,
    },
    /// Stops a delegation (`inputs[1]`) earning, pays out its rewards and
    /// starts its unbonding (spec §5.46).
    Undelegate,
    /// Withdraws an unbonded delegation's stake (`inputs[1]`) (spec §5.47).
    WithdrawDelegation,
    /// Collects a delegation's rewards (`inputs[1]`) (spec §5.48).
    ClaimDelegatorRewards,
    /// The operator sets its pool's commission from the next epoch's end
    /// (spec §5.49).
    SetCommission {
        /// The validator.
        validator: ValidatorId,
        /// The commission, in thousandths.
        permille: u16,
    },
    /// Escrows `price × rounds` for keeping data whose Merkle root is
    /// `root` (spec §5.50).
    PostDeal {
        /// The Merkle tree hash over the data's leaf hashes.
        root: Hash32,
        /// Leaves.
        size: u64,
        /// Leaves challenged per round.
        challenges: u8,
        /// Paid per proven round.
        price: Amount,
        /// Rounds.
        rounds: u32,
        /// Pulses per round.
        round_length: u64,
        /// Collateral the provider posts when it accepts.
        collateral: Amount,
        /// The provider, or `None` for a deal anyone may accept.
        provider: Option<AccountId>,
        /// The last Pulse the deal may be accepted.
        accept_by: Pulse,
        /// Where providers can fetch the data (not checked).
        locator: String,
    },
    /// A provider accepts a deal with its collateral and gets a custody
    /// record (spec §5.51).
    AcceptDeal {
        /// The deal.
        deal: ObjectId,
    },
    /// Proves the current round of the deals whose custody records are
    /// `inputs[1..]`, one proof each, in order (spec §5.52).
    ProveStorage {
        /// One proof per custody record.
        proofs: Vec<CustodyProof>,
    },
    /// The provider collects for the rounds it proved (spec §5.53).
    SettleDeal {
        /// The deal.
        deal: ObjectId,
    },
    /// Closes a deal: refunds an open one, or ends an accepted one at its
    /// end or once abandoned (spec §5.54).
    CloseDeal {
        /// The deal.
        deal: ObjectId,
    },
    /// An agent commits to up to `MAX_COHORT` subagents by the root of a
    /// sum tree over them, carving the sums of their bursts and rates from
    /// its bucket (spec §5.55). Its allotment is `inputs[1]`.
    SpawnCohort {
        /// The parent's charter.
        charter: ObjectId,
        /// The sum tree's root hash.
        root: Hash32,
        /// How many members.
        count: u32,
        /// The sum of the members' bursts.
        burst: Amount,
        /// The sum of the members' rates.
        rate: Amount,
        /// Whether the members start with full buckets, their tokens taken
        /// from the parent's now.
        full: bool,
        /// The members' caveats, each member's own burst and rate in place
        /// of these, which bound them.
        caveats: Caveats,
    },
    /// The parent of a cohort creates members of it, each with a proof of
    /// its leaf (spec §5.56). Its allotment is `inputs[1]`.
    MaterializeAgents {
        /// The cohort.
        cohort: ObjectId,
        /// The members, by increasing index, at most `MAX_MATERIALIZE`.
        members: Vec<Materialize>,
    },
    /// The agent or an overseer adds limits to an agent's whole subtree, or
    /// the backer widens or lifts them (spec §5.57).
    NarrowAgent {
        /// The charter.
        charter: ObjectId,
        /// The limits, or `None` to lift them.
        narrowing: Option<Narrowing>,
    },
}

/// A transaction body (what is signed).
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Transaction {
    /// The sender.
    pub sender: AccountId,
    /// The sender's genesis policy, required on its first transaction.
    pub genesis_policy: Option<AuthPolicy>,
    /// Owned inputs; `inputs[0]` is the gas coin (or the faucet's account object).
    pub inputs: Vec<ObjectRef>,
    /// Fee in USD.
    pub fee: Amount,
    /// What it does.
    pub kind: TxKind,
    /// Last Pulse in which it may execute.
    pub expires_at: Pulse,
    /// Makes otherwise identical transactions distinct.
    pub nonce: u64,
}

/// A transaction with its authorization (spec §4.3).
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct SignedTransaction {
    /// The body.
    pub tx: Transaction,
    /// Signatures satisfying the authorizing policy: over the body's digest,
    /// or, for a batch member, over its batch's [`TxBatch::message`].
    pub auth: Authorization,
    /// For a batch member, its batch and its place in it.
    #[serde(default)]
    pub batch: Option<BatchMember>,
}

impl Transaction {
    /// The digest (also the signed message).
    pub fn digest(&self) -> TxDigest {
        TxDigest::derive(&encode(self))
    }

    /// Signs with a dual keypair under its default policy.
    /// Signs for the chain with configuration `config`.
    pub fn sign(
        self,
        keys: &DualKeypair,
        config: &ChainConfig,
    ) -> Result<SignedTransaction, CryptoError> {
        let auth = keys.authorize(&signing_context(config), &self.digest().0 .0)?;
        Ok(SignedTransaction {
            tx: self,
            auth,
            batch: None,
        })
    }

    /// The contention domain a transaction must be ordered in, if any.
    pub fn domain(&self) -> Option<DomainId> {
        match &self.kind {
            TxKind::Accept { .. }
            | TxKind::Release { .. }
            | TxKind::Challenge { .. }
            | TxKind::Rule { .. }
            | TxKind::WithdrawBond { .. } => Some(well_known::assurance_domain()),
            TxKind::RevokeCharter { charter }
            | TxKind::ReclaimCharter { charter }
            | TxKind::SetAgentCard { charter, .. }
            | TxKind::SuspendAgent { charter }
            | TxKind::ResumeAgent { charter }
            | TxKind::NarrowAgent { charter, .. } => Some(object_domain("charter", charter)),
            TxKind::Rebalance { bounded, .. } => Some(object_domain("bounded", bounded)),
            TxKind::RevealBatch { market, .. } | TxKind::ClearBatch { market, .. } => {
                Some(object_domain("market", market))
            }
            TxKind::RegisterValidator { .. }
            | TxKind::Unstake { .. }
            | TxKind::WithdrawStake { .. }
            | TxKind::ClaimRewards { .. }
            | TxKind::ReportEquivocation { .. }
            | TxKind::DelegateStake { .. }
            | TxKind::Undelegate
            | TxKind::WithdrawDelegation
            | TxKind::ClaimDelegatorRewards
            | TxKind::SetCommission { .. } => Some(well_known::validators_domain()),
            TxKind::SettleStream { stream, .. }
            | TxKind::TopUpStream { stream, .. }
            | TxKind::CloseStream { stream } => Some(object_domain("stream", stream)),
            TxKind::SettleTask { .. } => Some(well_known::assurance_domain()),
            TxKind::CancelTask { task } => Some(object_domain("task", task)),
            TxKind::AcceptDeal { deal }
            | TxKind::SettleDeal { deal }
            | TxKind::CloseDeal { deal } => Some(object_domain("deal", deal)),
            _ => None,
        }
    }

    /// The coordination class, derived from the operation.
    pub fn class(&self) -> CoordinationClass {
        if self.domain().is_some() {
            return CoordinationClass::Contended;
        }
        match self.kind {
            TxKind::IssueClaim { .. } | TxKind::RegisterAtom { .. } | TxKind::PostBond { .. } => {
                CoordinationClass::Knowledge
            }
            TxKind::AgentPay { .. }
            | TxKind::SpendAllotment { .. }
            | TxKind::ReturnAllotment
            | TxKind::Delegate { .. }
            | TxKind::SpawnAgents { .. }
            | TxKind::SpawnCohort { .. }
            | TxKind::MaterializeAgents { .. }
            | TxKind::PostTask {
                charter: Some(_), ..
            } => CoordinationClass::Bounded,
            _ => CoordinationClass::OwnedLinear,
        }
    }

    /// Encoded size, for fees and bandwidth accounting.
    pub fn size(&self) -> usize {
        encode(self).len()
    }
}

impl SignedTransaction {
    /// The digest.
    pub fn digest(&self) -> TxDigest {
        self.tx.digest()
    }

    /// Total encoded size including signatures.
    pub fn size(&self) -> usize {
        encode(self).len()
    }
}

/// The domain of a shared object created by kind `kind`.
pub fn object_domain(kind: &str, object: &ObjectId) -> DomainId {
    DomainId::derive_parts(&[kind.as_bytes(), &object.0 .0])
}

#[cfg(test)]
mod tests {
    use super::*;
    use sion_core::ids::Version;

    fn tx(kind: TxKind) -> Transaction {
        Transaction {
            sender: AccountId::derive(b"s"),
            genesis_policy: None,
            inputs: vec![ObjectRef {
                id: ObjectId::derive(b"gas"),
                version: Version(0),
            }],
            fee: Amount::units(1),
            kind,
            expires_at: Pulse(10),
            nonce: 0,
        }
    }

    #[test]
    fn classes_are_derived() {
        let transfer = tx(TxKind::Transfer {
            asset: AssetId::usd(),
            outputs: vec![],
        });
        assert_eq!(transfer.class(), CoordinationClass::OwnedLinear);
        let atom = tx(TxKind::RegisterAtom {
            class: AtomClass::Code,
            label: "circuit".into(),
        });
        assert_eq!(atom.class(), CoordinationClass::Knowledge);
        let accept = tx(TxKind::Release {
            reservation: Hash32::ZERO,
        });
        assert_eq!(accept.class(), CoordinationClass::Contended);
        assert_eq!(accept.domain(), Some(well_known::assurance_domain()));
        let pay = tx(TxKind::SpendAllotment {
            to: AccountId::derive(b"x"),
            amount: Amount::units(1),
        });
        assert_eq!(pay.class(), CoordinationClass::Bounded);
    }

    #[test]
    fn digests_change_with_any_field() {
        let a = tx(TxKind::ReturnAllotment);
        let mut b = a.clone();
        b.nonce = 1;
        assert_ne!(a.digest(), b.digest());
    }

    #[test]
    fn a_batch_lowers_the_fee_floor_by_the_shared_signature_check() {
        // Devnet: 1,000 base and 100 per KiB, so a 300-byte body costs
        // 1,100 signed by itself; 800 of the base pays for the check.
        let config = ChainConfig::devnet(None);
        let alone = fee_floor(&config, 300, None).unwrap();
        assert_eq!(alone, config.min_fee(300));
        assert_eq!(alone, Amount(1_100));
        assert_eq!(fee_floor(&config, 300, Some(1)).unwrap(), alone);
        assert_eq!(
            fee_floor(&config, 300, Some(256)).unwrap(),
            Amount(1_100 - 800 + 4)
        );
        // The member's part rounds up: 800 / 3 is 266.67.
        assert_eq!(
            fee_floor(&config, 300, Some(3)).unwrap(),
            Amount(1_100 - 800 + 267)
        );
        // A declared size of zero is never valid; its floor counts it as one.
        assert_eq!(fee_floor(&config, 300, Some(0)).unwrap(), alone);
        // The share overflowing makes the floor undefined.
        let absurd = ChainConfig {
            base_fee: Amount(u128::MAX / 2),
            fee_per_kib: Amount::ZERO,
            ..config
        };
        assert_eq!(fee_floor(&absurd, 300, Some(2)), None);
    }

    #[test]
    fn an_envelope_carries_a_whole_batch_and_nothing_else() {
        let keys = DualKeypair::from_seed(&[5; 32], sion_crypto::Scheme::SlhDsaSha2_128f);
        let signer = keys.policy().account_id();
        let config = ChainConfig::devnet(None);
        let bodies: Vec<_> = (0..3)
            .map(|n| {
                let mut body = tx(TxKind::ReturnAllotment);
                body.sender = signer;
                body.nonce = n;
                (body, None)
            })
            .collect();
        let members = sign_batch(&keys, signer, &config, bodies).unwrap();
        assert!(members.iter().all(|m| m.auth == members[0].auth));
        let envelope = BatchEnvelope::of(&members).unwrap();
        assert_eq!(envelope.clone().members().unwrap(), members);
        // Part of a batch, or its members out of order, is no envelope.
        assert!(BatchEnvelope::of(&members[..2]).is_none());
        let swapped = vec![members[1].clone(), members[0].clone(), members[2].clone()];
        assert!(BatchEnvelope::of(&swapped).is_none());
        // A member replaced, removed or added does not give the root.
        let mut edited = envelope.clone();
        edited.members[1].0.nonce = 9;
        assert!(edited.members().is_err());
        let mut short = envelope.clone();
        short.members.pop();
        assert!(short.members().is_err());
        let mut empty = envelope;
        empty.members.clear();
        assert!(empty.members().is_err());
        // No batch of none, nor of more than the limit.
        assert!(sign_batch(&keys, signer, &config, Vec::new()).is_err());
    }
}
