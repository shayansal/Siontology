//! Bounded shared totals (the escrow method and demarcation protocol).
//!
//! A shared total — seats, a budget, a rate limit — is split into
//! single-writer allotments that sum to at most the total. Each writer
//! spends its allotment with ordinary lane certificates and never
//! coordinates with the others; only rebalancing goes through consensus.
//! The invariant "never exceed the total" holds throughout.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use fold_core::ids::{AccountId, AssetId, ObjectId};
use fold_core::Amount;
use serde::{Deserialize, Serialize};

/// A bounded total (a shared object).
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
    /// Sum of all allotments ever issued.
    pub fn allotted(&self) -> Amount {
        Amount(self.allotments.values().map(|(_, amount)| amount.0).sum())
    }

    /// The invariant: issued allotments plus what is unallocated never
    /// exceed the total plus what was returned.
    pub fn invariant_holds(&self) -> bool {
        self.allotted().0 + self.unallocated.0 <= self.total.0 + self.returned.0
    }
}
