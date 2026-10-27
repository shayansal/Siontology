//! The assurance contention domain: the registry plus the funds it holds.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sion_assurance::Registry;
use sion_core::hash::Hash32;
use sion_core::ids::AccountId;
use sion_core::Amount;

/// The assurance domain object: the registry and its vault. Bonds,
/// challenge deposits and slashing pools are all held here, so every
/// payout is a write to this one domain.
#[derive(
    Clone, Debug, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub struct AssuranceDomain {
    /// The registry.
    #[serde(skip)]
    pub registry: Registry,
    /// USD held for bonds, pools and deposits.
    pub vault: Amount,
    /// Open challenge deposits by challenge.
    pub deposits: BTreeMap<Hash32, (AccountId, Amount)>,
}
