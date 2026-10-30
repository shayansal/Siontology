//! Objects and their sorts.
//!
//! Every piece of mutable state is an object with a version and an owner.
//! The owner determines the coordination class of operations on it:
//! objects owned by one account change with a lane certificate alone;
//! shared objects belong to a contention domain and change in consensus
//! order. Knowledge (claims) is not an object at all: it is appended to the
//! grow-only knowledge log.

use std::collections::{BTreeMap, BTreeSet};

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sion_core::encode::encode;
use sion_core::hash::{hash_domain, Hash32};
use sion_core::ids::{AccountId, AssetId, DomainId, ObjectId, Pulse, TxDigest, Version};
use sion_core::Amount;

use crate::account::AccountState;
use crate::assurance::AssuranceDomain;
use crate::charter::{Allotment, Charter};
use crate::escrow::BoundedTotal;
use crate::market::Market;

/// Who may change an object.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
    Deserialize,
)]
pub enum Owner {
    /// One account: changes need only that account's signature and a lane certificate.
    Account(AccountId),
    /// A contention domain: changes are ordered by consensus.
    Shared(DomainId),
}

/// The coordination class of an object or operation, from the
/// architecture's Foundations section.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
    Deserialize,
)]
pub enum CoordinationClass {
    /// Grow-only knowledge: never conflicts, needs no ordering.
    Knowledge,
    /// Owned linear resources: one quorum certificate, no consensus.
    OwnedLinear,
    /// Shared but bounded totals, spent through single-writer allotments.
    Bounded,
    /// Shared and contended: consensus within the domain.
    Contended,
}

/// A coin: a linear amount of one asset.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Coin {
    /// The asset.
    pub asset: AssetId,
    /// The amount.
    pub amount: Amount,
}

/// What remains of a consumed object. Nothing is ever deleted.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Tombstone {
    /// The transaction that consumed it.
    pub consumed_by: TxDigest,
    /// When.
    pub at: Pulse,
}

/// Fees collected by the protocol, and the supply ledger.
#[derive(
    Clone, Debug, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub struct Treasury {
    /// Collected fees and released slashing pools, by asset.
    pub balances: BTreeMap<AssetId, Amount>,
    /// Total ever minted, by asset (the supply invariant's reference).
    pub minted: BTreeMap<AssetId, Amount>,
    /// Fees ever credited, by asset: a running total that only grows.
    /// Validators are paid a share of what it gained each epoch.
    pub fees: BTreeMap<AssetId, Amount>,
}

/// The data an object holds.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub enum ObjectData {
    /// An account's current authorization policy.
    Account(AccountState),
    /// A coin.
    Coin(Coin),
    /// A single-writer allotment of a bounded total or a charter's budget.
    Allotment(Allotment),
    /// A bounded shared total split into allotments.
    Bounded(BoundedTotal),
    /// An Agent Charter.
    Charter(Charter),
    /// A market that clears in certified batches.
    Market(Box<Market>),
    /// The assurance domain: registry and vault.
    Assurance(Box<AssuranceDomain>),
    /// The protocol treasury.
    Treasury(Treasury),
    /// Scheduled protocol events (batch sealing and deadlines).
    Schedule(Schedule),
    /// A consumed object.
    Tombstone(Tombstone),
    /// The validator set: stake, epochs and committees.
    Validators(Box<crate::validators::ValidatorSet>),
    /// Forced-inclusion obligations.
    Forced(Box<crate::validators::ForcedObligations>),
    /// A payment stream.
    Stream(crate::stream::Stream),
    /// A memory log's head (specification v1.9).
    MemoryLog(crate::memory::MemoryLog),
    /// An escrowed task (specification v1.10).
    Task(Box<crate::task::Task>),
    /// Every validator's pool of delegated stake (specification v1.11).
    Delegations(Box<crate::delegation::DelegationBook>),
    /// A delegation, owned by its delegator (specification v1.11).
    Delegation(crate::delegation::Delegation),
    /// A storage deal (specification v1.12).
    Deal(Box<crate::storage::Deal>),
    /// A provider's custody record of a storage deal, owned by the
    /// provider (specification v1.12).
    Custody(crate::storage::Custody),
    /// The beacon of a recorded Pulse, in one of the reusable slots
    /// (specification v1.12, §6.3).
    Beacon(crate::beacons::BeaconRecord),
    /// What happened to an agent after it was created: revoked, suspended,
    /// narrowed (specification v1.14, §5.13).
    AgentOverlay(crate::charter::AgentOverlay),
    /// A cohort of subagents committed to by a sum tree (specification
    /// v1.14, §5.55).
    Cohort(Box<crate::cohort::Cohort>),
}

/// Something the protocol must do at a given Pulse.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
    Deserialize,
)]
pub enum ScheduledEvent {
    /// Seal a market batch at the end of its window.
    SealBatch {
        /// Market.
        market: ObjectId,
        /// Batch.
        batch: u64,
    },
    /// Refund a sealed batch nobody cleared in time.
    BatchDeadline {
        /// Market.
        market: ObjectId,
        /// Batch.
        batch: u64,
    },
    /// Advance the assurance registry's clock: expire claims whose window
    /// ended, release reservations whose reliance ended and free pool funds
    /// no longer needed. Scheduled only for Pulses where something is due,
    /// so quiet Pulses never touch the registry.
    RegistryTick,
    /// Check that a forced intent's shard made it unexecutable in time.
    ForcedDeadline {
        /// The intent.
        intent: sion_core::ids::TxDigest,
    },
    /// End the current epoch: pay rewards, admit and retire validators, and
    /// sample the next epoch's committees.
    EpochEnd,
}

/// The protocol's schedule (a shared object). Every time-driven protocol
/// action is an entry here, so the end of a Pulse reads one small object to
/// learn what is due instead of scanning the state.
#[derive(
    Clone, Debug, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub struct Schedule {
    /// Events by the Pulse at whose end they fire.
    pub due: BTreeMap<u64, BTreeSet<ScheduledEvent>>,
}

/// An object in the state tree.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Object {
    /// Identifier.
    pub id: ObjectId,
    /// Version, incremented on every write.
    pub version: Version,
    /// Owner.
    pub owner: Owner,
    /// Data.
    pub data: ObjectData,
}

impl Object {
    /// Digest committed in the state tree.
    pub fn digest(&self) -> Hash32 {
        hash_domain("sion/exec/object", &encode(self))
    }

    /// The object's coordination class.
    pub fn class(&self) -> CoordinationClass {
        match (&self.owner, &self.data) {
            (_, ObjectData::Allotment(_)) => CoordinationClass::Bounded,
            (Owner::Account(_), _) => CoordinationClass::OwnedLinear,
            (Owner::Shared(_), _) => CoordinationClass::Contended,
        }
    }

    /// Whether the object has been consumed.
    pub fn is_tombstone(&self) -> bool {
        matches!(self.data, ObjectData::Tombstone(_))
    }

    /// A reference to this object at its current version.
    pub fn reference(&self) -> ObjectRef {
        ObjectRef {
            id: self.id,
            version: self.version,
        }
    }

    /// The coin, if this is one.
    pub fn as_coin(&self) -> Option<&Coin> {
        match &self.data {
            ObjectData::Coin(coin) => Some(coin),
            _ => None,
        }
    }
}

/// A versioned reference to an owned input: the lock key for lane
/// certificates.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
    Deserialize,
)]
pub struct ObjectRef {
    /// The object.
    pub id: ObjectId,
    /// The version the transaction consumes.
    pub version: Version,
}

/// Well-known object identifiers.
pub mod well_known {
    use super::*;

    /// The assurance registry object.
    pub fn registry() -> ObjectId {
        ObjectId::derive(b"well-known/assurance-registry")
    }

    /// The assurance contention domain.
    pub fn assurance_domain() -> DomainId {
        DomainId::derive(b"assurance")
    }

    /// The treasury object.
    pub fn treasury() -> ObjectId {
        ObjectId::derive(b"well-known/treasury")
    }

    /// The treasury's domain.
    pub fn treasury_domain() -> DomainId {
        DomainId::derive(b"treasury")
    }

    /// The schedule object.
    pub fn schedule() -> ObjectId {
        ObjectId::derive(b"well-known/schedule")
    }

    /// The schedule's domain.
    pub fn schedule_domain() -> DomainId {
        DomainId::derive(b"schedule")
    }

    /// The validator set.
    pub fn validators() -> ObjectId {
        ObjectId::derive(b"well-known/validators")
    }

    /// The validator set's contention domain.
    pub fn validators_domain() -> DomainId {
        DomainId::derive(b"validators")
    }

    /// The validators' pools of delegated stake (in the validators'
    /// domain; created by the first delegation).
    pub fn delegations() -> ObjectId {
        ObjectId::derive(b"well-known/delegations")
    }

    /// The `slot`-th beacon record (`crate::beacons`): it holds the beacon
    /// of the latest recorded Pulse whose slot it is.
    pub fn beacon(slot: u32) -> ObjectId {
        ObjectId::derive_parts(&[b"well-known/beacon", &slot.to_be_bytes()])
    }

    /// The beacon records' domain.
    pub fn beacons_domain() -> DomainId {
        DomainId::derive(b"beacons")
    }

    /// The forced-inclusion obligations.
    pub fn forced() -> ObjectId {
        ObjectId::derive(b"well-known/forced")
    }

    /// The obligations' domain.
    pub fn forced_domain() -> DomainId {
        DomainId::derive(b"forced")
    }

    /// The account object for `account`.
    pub fn account(account: &AccountId) -> ObjectId {
        ObjectId::derive_parts(&[b"account", &account.0 .0])
    }
}

/// Derives the identifier of the `index`-th object created by a transaction.
pub fn created_id(tx: &TxDigest, index: u32) -> ObjectId {
    ObjectId::derive_parts(&[b"created", &tx.0 .0, &index.to_be_bytes()])
}

/// The set of assets a slice of coins holds.
pub fn assets_of(coins: &[Coin]) -> BTreeSet<AssetId> {
    coins.iter().map(|c| c.asset).collect()
}
