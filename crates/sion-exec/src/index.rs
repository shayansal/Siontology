//! An index of claims (specification §5.58): each owner's latest claim
//! under a key, such as a publisher's latest release of a skill.
//!
//! An entry lives at [`index_id`]`(owner, key)`. `IndexClaim` creates it,
//! or moves it to a later claim of the owner's own: one issued after the
//! claim it names. A reader who checks the entry with a state proof against
//! a certified header therefore knows the owner's latest claim under the
//! key as of that header; a server can still answer from an older header,
//! and how recent a header must be is for the reader to require. The
//! entry only says which claim is the latest; whether that claim still
//! holds is the claim's own status, which a reader checks as it would any
//! claim's.

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sion_core::encode::encode;
use sion_core::hash::hash_domain;
use sion_core::ids::{AccountId, ClaimId, ObjectId, Pulse};

/// The longest key, in bytes.
pub const MAX_INDEX_KEY: usize = 64;

/// An owner's latest claim under a key (a shared object in its own
/// contention domain, at [`index_id`]).
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct IndexEntry {
    /// Whose claims it indexes.
    pub owner: AccountId,
    /// The key, such as `skill:pay-invoices`.
    pub key: String,
    /// The latest claim.
    pub claim: ClaimId,
    /// The claim it replaced, if any.
    pub previous: Option<ClaimId>,
    /// How many claims it has named, this one included.
    pub count: u64,
    /// When it last moved.
    pub updated_at: Pulse,
}

/// Where `owner`'s entry for `key` lives:
/// `ObjectId(H("sion/id/index", borsh((owner, key))))`.
pub fn index_id(owner: &AccountId, key: &str) -> ObjectId {
    ObjectId(hash_domain(
        "sion/id/index",
        &encode(&(*owner, key.to_string())),
    ))
}

/// Whether `key` can name an entry: 1 to [`MAX_INDEX_KEY`] bytes, each a
/// printable ASCII character other than space (`0x21` to `0x7e`).
pub fn valid_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= MAX_INDEX_KEY && key.bytes().all(|b| (0x21..=0x7e).contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_short_printable_and_without_spaces() {
        assert!(valid_key("skill:pay-invoices"));
        assert!(valid_key(&"k".repeat(MAX_INDEX_KEY)));
        assert!(!valid_key(""));
        assert!(!valid_key(&"k".repeat(MAX_INDEX_KEY + 1)));
        assert!(!valid_key("two words"));
        assert!(!valid_key("tab\there"));
        assert!(!valid_key("café"));
    }

    #[test]
    fn entries_live_apart_by_owner_and_key() {
        let (a, b) = (AccountId::derive(b"a"), AccountId::derive(b"b"));
        let id = index_id(&a, "skill:x");
        assert_eq!(id, index_id(&a, "skill:x"));
        assert_ne!(id, index_id(&b, "skill:x"));
        assert_ne!(id, index_id(&a, "skill:y"));
    }
}
