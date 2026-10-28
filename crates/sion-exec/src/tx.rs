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

use crate::charter::{Caveats, Spawn};
use crate::claim::ClaimBody;
use crate::market::Submission;
use crate::object::{well_known, CoordinationClass, ObjectRef};
use crate::state::ChainConfig;
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

/// The most owned inputs a transaction may name.
pub const MAX_INPUTS: usize = 256;

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

/// A transaction with its authorization.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct SignedTransaction {
    /// The body.
    pub tx: Transaction,
    /// Signatures satisfying the sender's policy.
    pub auth: Authorization,
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
        Ok(SignedTransaction { tx: self, auth })
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
            | TxKind::ResumeAgent { charter } => Some(object_domain("charter", charter)),
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
}
