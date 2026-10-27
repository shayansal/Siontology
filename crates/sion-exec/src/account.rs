//! Accounts: stable identifiers, rotatable policies.

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sion_core::ids::{AccountId, Pulse};
use sion_crypto::{AuthPolicy, KemPublicKey};

/// An account's current state. The account ID is the hash of the genesis
/// policy and never changes; `policy` is the current one.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct AccountState {
    /// The account.
    pub id: AccountId,
    /// The current authorization policy.
    pub policy: AuthPolicy,
    /// An ML-KEM key for receiving encrypted payloads (viewing key).
    pub viewing_key: Option<KemPublicKey>,
    /// When the account first acted.
    pub created_at: Pulse,
    /// How many times the policy has rotated.
    pub rotations: u32,
}
