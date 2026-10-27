//! Markets that clear in certified batches (Verifiable Clearing).
//!
//! Orders join the batch for the current window without consensus (adding
//! to a set commutes); when the window ends the batch seals with the
//! Pulse's beacon; solvers then submit clearings, and the first one that
//! verifies (certificate, canonical tie-break, canonical price) settles it in
//! consensus order. Everyone in the batch trades at one price, so a position
//! inside the batch is worth nothing.
//!
//! Sealed orders are encrypted to the market's committee
//! ([`fold_crypto::CommitteeEnvelope`]); they are revealed after sealing by
//! any `t` members' key shares.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use fold_clearing::{Order, Side};
use fold_core::hash::Hash32;
use fold_core::ids::{AccountId, AssetId, DomainId, Pulse};
use fold_core::Amount;
use fold_crypto::{CommitteeEnvelope, KemPublicKey};
use serde::{Deserialize, Serialize};

/// An order's terms before sealing.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub struct OrderTerms {
    /// Buy or sell.
    pub side: Side,
    /// Units of the good.
    pub quantity: u64,
    /// Price per unit in micro-units of the numeraire.
    pub limit_price: Amount,
}

impl OrderTerms {
    /// The escrow an order needs: numeraire for buys (quantity × limit),
    /// the good for sells (quantity units).
    pub fn escrow(&self, market: &Market) -> (AssetId, Amount) {
        match self.side {
            Side::Buy => (
                market.numeraire,
                Amount(self.limit_price.0.saturating_mul(u128::from(self.quantity))),
            ),
            Side::Sell => (market.good, Amount::units(self.quantity)),
        }
    }
}

/// How an order is submitted.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub enum Submission {
    /// In the clear.
    Plain(OrderTerms),
    /// Encrypted to the market committee; the escrow is public.
    Sealed {
        /// The encrypted terms.
        envelope: CommitteeEnvelope,
        /// Public escrow: which side and how much is locked.
        side: Side,
        /// Escrowed amount.
        escrow: Amount,
    },
}

/// An order waiting in a batch.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct PendingOrder {
    /// Order identifier.
    pub id: Hash32,
    /// Owner.
    pub owner: AccountId,
    /// Escrowed asset.
    pub escrow_asset: AssetId,
    /// Escrowed amount.
    pub escrow: Amount,
    /// Terms, once known (immediately for plain orders, after reveal for sealed ones).
    pub terms: Option<OrderTerms>,
    /// The sealed envelope, for sealed orders.
    pub sealed: Option<CommitteeEnvelope>,
}

/// A batch's lifecycle.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub enum BatchStatus {
    /// Accepting orders.
    Open,
    /// Sealed with a beacon; waiting for reveal (if sealed orders) and clearing.
    Sealed {
        /// Beacon value revealed at sealing.
        beacon: Hash32,
        /// Pulse of sealing.
        at: Pulse,
    },
    /// Settled.
    Cleared {
        /// Uniform price, if anything traded.
        price: Option<Amount>,
        /// Units traded.
        volume: u64,
        /// Gains from trade.
        welfare: Amount,
        /// The solver whose clearing won.
        solver: AccountId,
    },
    /// Not cleared in time; every order refunded.
    Refunded,
}

/// One batch.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct MarketBatch {
    /// Batch index (Pulse / batch length).
    pub index: u64,
    /// Orders sorted by id.
    pub orders: Vec<PendingOrder>,
    /// Status.
    pub status: BatchStatus,
}

/// A market (a shared object in its own contention domain).
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Market {
    /// Name.
    pub label: String,
    /// The contention domain.
    pub domain: DomainId,
    /// The good traded (for example GPU-hour vouchers).
    pub good: AssetId,
    /// The numeraire (for example USD).
    pub numeraire: AssetId,
    /// Pulses per batch.
    pub batch_length: u64,
    /// Pulses after sealing within which a clearing must land.
    pub clear_deadline: u64,
    /// Committee keys for sealed orders (empty: plain only).
    pub committee: Vec<KemPublicKey>,
    /// Shares needed to reveal sealed orders.
    pub threshold: u8,
    /// Batches by index.
    pub batches: BTreeMap<u64, MarketBatch>,
}

impl Market {
    /// The batch index for a Pulse.
    pub fn batch_index(&self, pulse: Pulse) -> u64 {
        pulse.0 / self.batch_length.max(1)
    }

    /// Converts a sealed batch's revealed orders into a clearing batch.
    pub fn clearing_batch(
        &self,
        batch: &MarketBatch,
        beacon: Hash32,
        pulse: Pulse,
    ) -> fold_clearing::Batch {
        let orders = batch
            .orders
            .iter()
            .filter_map(|pending| {
                pending.terms.map(|terms| Order {
                    id: pending.id,
                    owner: pending.owner,
                    side: terms.side,
                    quantity: terms.quantity,
                    limit_price: terms.limit_price,
                })
            })
            .collect();
        fold_clearing::Batch {
            domain: self.domain,
            pulse,
            beacon,
            orders,
        }
    }
}

/// The associated data binding a sealed order to its market and batch.
pub fn sealing_aad(domain: &DomainId, batch: u64) -> Vec<u8> {
    let mut out = domain.0 .0.to_vec();
    out.extend_from_slice(&batch.to_be_bytes());
    out
}

/// Domain tag for sealed orders.
pub const SEALED_ORDER_DOMAIN: &str = "fold/market/sealed-order";
