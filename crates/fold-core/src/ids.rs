//! Newtype identifiers.
//!
//! Each identifier is its own type, so the compiler rejects passing a claim
//! ID where an account ID is expected. Digest-based identifiers carry the
//! domain tag used to derive them.

use std::fmt;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};

use crate::hash::{hash_domain, hash_parts, Hash32};

macro_rules! digest_id {
    ($(#[$meta:meta])* $name:ident, $domain:literal) => {
        $(#[$meta])*
        #[derive(
            Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default,
            BorshSerialize, BorshDeserialize, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub Hash32);

        impl $name {
            /// The domain tag used to derive this identifier.
            pub const DOMAIN: &'static str = $domain;

            /// Wraps an existing digest.
            pub const fn from_hash(hash: Hash32) -> Self {
                Self(hash)
            }

            /// Derives an identifier from canonical bytes.
            pub fn derive(bytes: &[u8]) -> Self {
                Self(hash_domain(Self::DOMAIN, bytes))
            }

            /// Derives an identifier from several length-prefixed parts.
            pub fn derive_parts(parts: &[&[u8]]) -> Self {
                Self(hash_parts(Self::DOMAIN, parts))
            }

            /// The underlying digest.
            pub fn as_hash(&self) -> &Hash32 {
                &self.0
            }

            /// Short hex form for logs.
            pub fn short(&self) -> String {
                self.0.short()
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self.0.short())
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, f)
            }
        }
    };
}

digest_id!(
    /// An account: the hash of its genesis authorization policy. It never
    /// changes when keys or schemes rotate.
    AccountId,
    "fold/id/account"
);
digest_id!(
    /// An object in the state tree (coins, bonds, allotments, charters, markets).
    ObjectId,
    "fold/id/object"
);
digest_id!(
    /// A claim: the hash of its canonical body.
    ClaimId,
    "fold/id/claim"
);
digest_id!(
    /// One irreducible trust assumption.
    AtomId,
    "fold/id/atom"
);
digest_id!(
    /// A transaction: the hash of its signed-over body.
    TxDigest,
    "fold/id/tx"
);
digest_id!(
    /// A consensus block (DAG vertex) digest.
    BlockDigest,
    "fold/id/block"
);
digest_id!(
    /// A contention domain: a set of shared objects ordered together.
    DomainId,
    "fold/id/domain"
);
digest_id!(
    /// An asset (for example, a stablecoin).
    AssetId,
    "fold/id/asset"
);
digest_id!(
    /// A validator: the hash of its consensus keys.
    ValidatorId,
    "fold/id/validator"
);

impl AssetId {
    /// The devnet's USD-denominated stablecoin.
    pub fn usd() -> Self {
        AssetId::derive(b"USD")
    }
}

macro_rules! number_id {
    ($(#[$meta:meta])* $name:ident, $inner:ty) => {
        $(#[$meta])*
        #[derive(
            Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default,
            BorshSerialize, BorshDeserialize, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub $inner);

        impl $name {
            /// The next value.
            pub fn next(self) -> Self {
                Self(self.0 + 1)
            }

            /// The raw value.
            pub fn get(self) -> $inner {
                self.0
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self.0)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, f)
            }
        }

        impl From<$inner> for $name {
            fn from(value: $inner) -> Self {
                Self(value)
            }
        }
    };
}

number_id!(
    /// The cadence at which the network emits a World Proof.
    Pulse,
    u64
);
number_id!(
    /// A consensus round.
    Round,
    u64
);
number_id!(
    /// A validator-set epoch.
    Epoch,
    u64
);
number_id!(
    /// An object version; every owned write increments it.
    Version,
    u64
);
number_id!(
    /// A shard index.
    ShardId,
    u16
);
number_id!(
    /// Wall-clock time in milliseconds since the Unix epoch (world time, used
    /// for claim validity windows; ledger time is the [`Pulse`]).
    TimestampMs,
    u64
);

impl ShardId {
    /// Assigns an account's owned objects to a shard.
    pub fn for_account(account: &AccountId, shard_count: u16) -> ShardId {
        assert!(shard_count > 0, "at least one shard");
        ShardId((account.0.prefix_u64() % u64::from(shard_count)) as u16)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_from_same_bytes_differ_by_type() {
        let account = AccountId::derive(b"x");
        let claim = ClaimId::derive(b"x");
        assert_ne!(account.0, claim.0);
    }

    #[test]
    fn shard_assignment_is_stable_and_in_range() {
        let account = AccountId::derive(b"alice");
        let shard = ShardId::for_account(&account, 4);
        assert!(shard.0 < 4);
        assert_eq!(shard, ShardId::for_account(&account, 4));
    }

    #[test]
    fn numbers_increment() {
        assert_eq!(Pulse(4).next(), Pulse(5));
        assert_eq!(Version(0).next().get(), 1);
    }
}
