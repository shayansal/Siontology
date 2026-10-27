//! Queries a validator answers and the views it returns.

use fold_assurance::Policy;
use fold_core::hash::Hash32;
use fold_core::ids::{AccountId, ClaimId, ObjectId, Pulse, TxDigest, ValidatorId};
use fold_core::Amount;
use fold_exec::{Event, ExecStatus, Object, TxEffects, WorldCommitment};
use fold_light::{PulseCertificate, PulseHeader};
use serde::{Deserialize, Serialize};

/// A query to a validator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Query {
    /// Chain status.
    Status,
    /// An account's coins and balances.
    Account(AccountId),
    /// An object with its proof against the latest certified state.
    Object(ObjectId),
    /// A claim with its status, cost of corruption and inclusion proof.
    Claim(ClaimId),
    /// The decision rule for a claim.
    Decide {
        /// The claim.
        claim: ClaimId,
        /// Value at risk.
        value: Amount,
        /// The consumer's policy.
        policy: Policy,
    },
    /// A transaction's execution record.
    Tx(TxDigest),
    /// A Pulse's summary (`None`: the latest).
    Pulse(Option<Pulse>),
    /// A Pulse's data-availability header.
    Data(Pulse),
    /// One share of a Pulse's data square.
    Share {
        /// The Pulse.
        pulse: Pulse,
        /// Row.
        row: usize,
        /// Column.
        column: usize,
    },
    /// Events from a Pulse on.
    Events {
        /// First Pulse.
        from: Pulse,
        /// Most events returned.
        limit: usize,
    },
    /// The committee and its keys.
    Validators,
    /// The World Proof of a Pulse.
    World(Pulse),
}

/// Chain status.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusView {
    /// Chain identifier.
    pub chain_id: String,
    /// Genesis digest.
    pub genesis: Hash32,
    /// The answering validator.
    pub validator: ValidatorId,
    /// Consensus epoch.
    pub epoch: u64,
    /// Last executed Pulse.
    pub pulse: Pulse,
    /// Last Pulse with a complete committee certificate.
    pub certified_pulse: Pulse,
    /// Current world commitment.
    pub commitment: WorldCommitment,
    /// Consensus round.
    pub round: u64,
    /// Anchors committed.
    pub anchors: u64,
    /// Transactions executed (successful, failed, rejected).
    pub executed: (u64, u64, u64),
    /// Claims in the knowledge log.
    pub claims: u64,
    /// Objects in state.
    pub objects: u64,
    /// Number of validators.
    pub validators: usize,
    /// Number of lane shards.
    pub shards: u16,
    /// Whether validators use the insecure simulation MAC.
    pub insecure_signatures: bool,
    /// Latest beacon value.
    pub beacon: Hash32,
    /// Last Pulse with a verified World Proof, if proving is on.
    pub proven_pulse: Option<Pulse>,
}

/// One coin.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoinView {
    /// The coin object.
    pub object: Object,
}

/// An account.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountView {
    /// The account.
    pub account: AccountId,
    /// The account object, if the account exists on chain.
    pub object: Option<Object>,
    /// Live coins.
    pub coins: Vec<Object>,
    /// Balances by asset.
    pub balances: Vec<(fold_core::ids::AssetId, Amount)>,
    /// Allotments the account can spend.
    pub allotments: Vec<Object>,
    /// The Pulse this view is from.
    pub pulse: Pulse,
}

/// A transaction's record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TxView {
    /// The Pulse it executed in.
    pub pulse: Pulse,
    /// Its position in the Pulse.
    pub index: u64,
    /// Its effects.
    pub effects: TxEffects,
    /// The outcome.
    pub status: ExecStatus,
    /// Receipt inclusion proof (against the Pulse header's receipts root).
    pub receipt: fold_light::ReceiptProof,
}

/// A Pulse's summary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PulseView {
    /// The header this validator computed.
    pub header: PulseHeader,
    /// The committee certificate, once complete.
    pub certificate: Option<PulseCertificate>,
    /// Transactions executed.
    pub transactions: Vec<TxDigest>,
    /// Outcome counts (success, failed, rejected).
    pub counts: (usize, usize, usize),
    /// Events of the deferred phase.
    pub deferred_events: Vec<Event>,
    /// Claims that joined the knowledge log.
    pub new_claims: Vec<ClaimId>,
    /// Anchors committed in the Pulse.
    pub anchors: u64,
    /// Validators whose beacon reveals counted.
    pub revealed: usize,
    /// Bytes of published data.
    pub data_bytes: usize,
    /// Execution time in milliseconds.
    pub execution_ms: u64,
}

/// An event with where it happened.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventView {
    /// The Pulse.
    pub pulse: Pulse,
    /// The transaction, if any (deferred-phase events have none).
    pub tx: Option<TxDigest>,
    /// The event.
    pub event: Event,
}

/// How a submitted transaction is being processed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TxStatus {
    /// Collecting lane approvals.
    Certifying {
        /// The shard.
        shard: u16,
        /// Approvals so far.
        approvals: usize,
    },
    /// Certified (owned) or submitted (contended); waiting for a Pulse.
    Sequencing,
    /// Certification failed.
    Rejected(String),
    /// Executed.
    Executed(Box<TxView>),
}

/// The answer to a submission.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmitReceipt {
    /// The transaction.
    pub digest: TxDigest,
    /// `lane` (owned path) or `consensus` (contended).
    pub route: String,
    /// The certifying shard (owned path).
    pub shard: Option<u16>,
}
