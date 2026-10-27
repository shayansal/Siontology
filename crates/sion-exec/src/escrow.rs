//! Bounded shared totals (the escrow method and demarcation protocol).
//!
//! A shared total — seats, a budget, a rate limit — is split into
//! single-writer allotments that sum to at most the total. Each writer
//! spends its allotment with ordinary lane certificates and never
//! coordinates with the others; only rebalancing goes through consensus.
//! The invariant "never exceed the total" holds throughout.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sion_core::ids::{AccountId, AssetId, ObjectId};
use sion_core::Amount;

/// A bounded total (a shared object).
/// The most allotments one bounded total may be created with (spec §5.18).
pub const MAX_ALLOTMENTS: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct BoundedTotal {
    /// What the total counts.
    pub label: String,
    /// Asset held in escrow for the total.
    pub asset: AssetId,
    /// Who may rebalance.
    pub manager: AccountId,
    /// The total.
    pub total: Amount,
    /// Not yet allotted.
    pub unallocated: Amount,
    /// Allotments issued, by object, with their writers and initial amounts.
    pub allotments: BTreeMap<ObjectId, (AccountId, Amount)>,
    /// Amounts returned by writers.
    pub returned: Amount,
}

impl BoundedTotal {
    /// Sum of all allotments ever issued (saturating).
    pub fn allotted(&self) -> Amount {
        self.allotments
            .values()
            .fold(Amount::ZERO, |sum, (_, amount)| sum.saturating_add(*amount))
    }

    /// The invariant: issued allotments plus what is unallocated never
    /// exceed the total plus what was returned.
    pub fn invariant_holds(&self) -> bool {
        let issued = self.allotted().0.checked_add(self.unallocated.0);
        let bound = self.total.0.checked_add(self.returned.0);
        match (issued, bound) {
            (Some(issued), Some(bound)) => issued <= bound,
            (_, None) => true,
            (None, Some(_)) => false,
        }
    }
}
