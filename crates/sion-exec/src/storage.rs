//! Storage deals (specification §5.50 to §5.54): paying a provider, round
//! by round, for keeping data and proving it still holds it.
//!
//! A client fixes the data by its Merkle root: the data is `size`
//! **leaves**, byte strings of at most [`MAX_LEAF_BYTES`] bytes, and the
//! root is `sion_core::merkle::root` over their leaf hashes (RFC 9162
//! shape). The client escrows `price × rounds` in a **deal**, a shared
//! object in its own domain, for a named provider or for any
//! ([`TxKind::PostDeal`](crate::tx::TxKind::PostDeal)). A provider that
//! holds the data accepts with collateral and gets a **custody record** it
//! owns ([`Custody`]).
//!
//! In each round the provider proves, from its custody record, that it
//! holds the leaves a **challenge** picks: [`challenge_index`] over a seed
//! that the beacon of the Pulse of every proof changes, so nobody knows a
//! round's challenge before the round before it is proven. Proving touches
//! only the provider's own records, so it needs a lane certificate, not
//! consensus, and one transaction proves up to [`MAX_PROOF_LEAVES`] leaves
//! across many deals. Money moves only when the provider collects for the
//! rounds it proved (`SettleDeal`) and when the deal is closed
//! (`CloseDeal`): the client gets back the escrow of every round not
//! proven, and the provider its collateral less `⌊bond / rounds⌋` per
//! missed round, or none of it if it abandoned the deal for
//! [`ABANDON_ROUNDS`] rounds. Forfeited collateral goes to the treasury.
//!
//! **What a proof shows.** That the provider could produce the challenged
//! leaves during the round. It does not show that the provider keeps a copy
//! of its own: a provider could fetch the leaves from someone else when
//! challenged (there is no proof of replication). Many leaves lost are
//! likely to be caught: a provider missing a fraction `f` of the leaves
//! answers a round with probability `(1 − f)^challenges`.
//!
//! This module defines the types and constants of the specification, the
//! rules' helpers ([`challenge_index`], [`first_seed`], [`next_seed`],
//! [`current_round`], [`Deal::end`]) and, for clients and providers off the
//! ledger, [`split_leaves`] and [`DataTree`]. The rules are in
//! [`crate::execute`].

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sion_core::hash::{hash_parts, Hash32};
use sion_core::ids::{AccountId, ObjectId, Pulse};
use sion_core::{merkle, Amount};

/// The most leaves a deal may cover (2⁴⁰).
pub const MAX_DEAL_LEAVES: u64 = 1 << 40;

/// The longest leaf, in bytes.
pub const MAX_LEAF_BYTES: usize = 4096;

/// The longest Merkle path a leaf proof may carry (enough for
/// [`MAX_DEAL_LEAVES`]).
pub const MAX_PATH: usize = 40;

/// The most leaves a round may challenge.
pub const MAX_CHALLENGES: u8 = 16;

/// The most rounds a deal may have.
pub const MAX_ROUNDS: u32 = 1_000_000;

/// The shortest round, in Pulses: long enough for a proof to be included.
pub const MIN_ROUND_LENGTH: u64 = 8;

/// The longest round, in Pulses.
pub const MAX_ROUND_LENGTH: u64 = 1_000_000;

/// The longest locator, in bytes.
pub const MAX_LOCATOR: usize = 256;

/// The most leaves one `ProveStorage` may prove, across its deals.
pub const MAX_PROOF_LEAVES: usize = 64;

/// How many rounds in a row a provider may leave unproven before the
/// client may close the deal and its collateral is forfeited.
pub const ABANDON_ROUNDS: u32 = 3;

/// The hash domain of a custody record's seed.
pub const SEED_DOMAIN: &str = "sion/storage/seed";

/// The hash domain of a challenge.
pub const CHALLENGE_DOMAIN: &str = "sion/storage/challenge";

/// One challenged leaf: its bytes and its Merkle path, from the leaf up.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct LeafProof {
    /// The leaf's bytes (at most [`MAX_LEAF_BYTES`]).
    pub data: Vec<u8>,
    /// Its inclusion path in the deal's tree.
    pub path: Vec<Hash32>,
}

/// A provider's answer to one round of one deal.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct CustodyProof {
    /// The round (the one under way when the proof executes).
    pub round: u32,
    /// One leaf per challenge, in challenge order.
    pub leaves: Vec<LeafProof>,
}

/// Where a deal stands.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub enum DealState {
    /// Waiting for a provider.
    Open,
    /// Accepted: rounds run from `start`.
    Active {
        /// The provider.
        provider: AccountId,
        /// Its custody record.
        custody: ObjectId,
        /// The first Pulse of round 0.
        start: Pulse,
        /// The collateral held.
        bond: Amount,
        /// Rounds paid for so far.
        paid: u32,
    },
    /// Closed, once and for all.
    Closed {
        /// When.
        at: Pulse,
        /// Rounds proven.
        proven: u32,
        /// Rounds missed.
        missed: u32,
        /// Escrow returned to the client.
        refund: Amount,
        /// Collateral forfeited to the treasury.
        forfeited: Amount,
    },
}

/// A storage deal: a shared object in its own contention domain.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Deal {
    /// Who pays.
    pub client: AccountId,
    /// The provider named, or `None` for a deal anyone may accept.
    pub provider: Option<AccountId>,
    /// The Merkle tree hash over the leaves' hashes.
    pub root: Hash32,
    /// Leaves.
    pub size: u64,
    /// Leaves challenged per round.
    pub challenges: u8,
    /// Paid per proven round.
    pub price: Amount,
    /// Rounds.
    pub rounds: u32,
    /// Pulses per round.
    pub round_length: u64,
    /// Collateral the provider posts when it accepts.
    pub collateral: Amount,
    /// The last Pulse the deal may be accepted.
    pub accept_by: Pulse,
    /// Where providers can fetch the data (not checked).
    pub locator: String,
    /// What is left in escrow.
    pub escrow: Amount,
    /// When it was posted.
    pub posted_at: Pulse,
    /// Where it stands.
    pub state: DealState,
}

impl Deal {
    /// The first Pulse after the deal's last round, once accepted.
    pub fn end(&self) -> Option<Pulse> {
        match self.state {
            DealState::Active { start, .. } => {
                Some(Pulse(start.0.saturating_add(
                    u64::from(self.rounds).saturating_mul(self.round_length),
                )))
            }
            _ => None,
        }
    }
}

/// A provider's custody record of one deal: owned by the provider, it
/// carries what proving needs and counts the rounds proven.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Custody {
    /// The deal.
    pub deal: ObjectId,
    /// The deal's root.
    pub root: Hash32,
    /// The deal's leaves.
    pub size: u64,
    /// Leaves challenged per round.
    pub challenges: u8,
    /// The first Pulse of round 0.
    pub start: Pulse,
    /// Pulses per round.
    pub round_length: u64,
    /// Rounds.
    pub rounds: u32,
    /// The seed the next challenge is drawn from.
    pub seed: Hash32,
    /// The first round not yet proven or passed over.
    pub next: u32,
    /// Rounds proven.
    pub proven: u32,
}

/// The seed of a newly accepted deal (§5.51 step 3).
pub fn first_seed(beacon: &Hash32, deal: &ObjectId) -> Hash32 {
    hash_parts(SEED_DOMAIN, &[&beacon.0, &deal.0 .0, &Hash32::ZERO.0])
}

/// The seed after a proof executed in a Pulse with `beacon` (§5.52).
pub fn next_seed(beacon: &Hash32, deal: &ObjectId, seed: &Hash32) -> Hash32 {
    hash_parts(SEED_DOMAIN, &[&beacon.0, &deal.0 .0, &seed.0])
}

/// The leaf the `k`-th challenge of `round` picks among `size` leaves.
pub fn challenge_index(seed: &Hash32, round: u32, k: u8, size: u64) -> u64 {
    let digest = hash_parts(CHALLENGE_DOMAIN, &[&seed.0, &round.to_be_bytes(), &[k]]);
    let value = u64::from_be_bytes(digest.0[..8].try_into().expect("8 bytes"));
    value % size.max(1)
}

/// The round under way at `pulse` for a deal whose rounds start at `start`,
/// if any has started.
pub fn current_round(start: Pulse, round_length: u64, pulse: Pulse) -> Option<u64> {
    (pulse >= start).then(|| (pulse.0 - start.0) / round_length.max(1))
}

/// Splits data into leaves of [`MAX_LEAF_BYTES`] (the last may be
/// shorter); empty data is one empty leaf.
pub fn split_leaves(data: &[u8]) -> Vec<&[u8]> {
    if data.is_empty() {
        vec![&data[..0]]
    } else {
        data.chunks(MAX_LEAF_BYTES).collect()
    }
}

/// The Merkle tree of a body of data, with every level kept so that a
/// provider answers a challenge with a path in `O(log size)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataTree {
    /// `levels[0]` are the leaf hashes; each level above pairs the one
    /// below, carrying an unpaired last node up unchanged (the RFC 9162
    /// shape).
    levels: Vec<Vec<Hash32>>,
}

impl DataTree {
    /// The tree of `data` split by [`split_leaves`].
    pub fn of_data(data: &[u8]) -> DataTree {
        Self::from_leaf_hashes(
            split_leaves(data)
                .into_iter()
                .map(merkle::leaf_hash)
                .collect(),
        )
    }

    /// The tree over leaf hashes (at least one).
    pub fn from_leaf_hashes(leaves: Vec<Hash32>) -> DataTree {
        let mut levels = vec![if leaves.is_empty() {
            vec![merkle::leaf_hash(b"")]
        } else {
            leaves
        }];
        while levels.last().expect("one level").len() > 1 {
            let below = levels.last().expect("one level");
            let above = below
                .chunks(2)
                .map(|pair| match pair {
                    [left, right] => merkle::node_hash(left, right),
                    [single] => *single,
                    _ => unreachable!("chunks of two"),
                })
                .collect();
            levels.push(above);
        }
        DataTree { levels }
    }

    /// The root: what a deal commits to.
    pub fn root(&self) -> Hash32 {
        self.levels.last().expect("one level")[0]
    }

    /// The number of leaves.
    pub fn size(&self) -> u64 {
        self.levels[0].len() as u64
    }

    /// The inclusion path of leaf `index`, from the leaf up.
    pub fn path(&self, index: u64) -> Option<Vec<Hash32>> {
        let mut position = usize::try_from(index).ok()?;
        if position >= self.levels[0].len() {
            return None;
        }
        let mut path = Vec::new();
        for level in &self.levels[..self.levels.len() - 1] {
            let sibling = position ^ 1;
            if sibling < level.len() {
                path.push(level[sibling]);
            }
            position /= 2;
        }
        Some(path)
    }

    /// Answers `round` of the deal recorded in `custody` from the data
    /// itself (the provider's copy).
    pub fn answer(&self, data: &[u8], custody: &Custody, round: u32) -> Option<CustodyProof> {
        let leaves = split_leaves(data);
        let mut proofs = Vec::with_capacity(usize::from(custody.challenges));
        for k in 0..custody.challenges {
            let index = challenge_index(&custody.seed, round, k, custody.size);
            proofs.push(LeafProof {
                data: leaves.get(usize::try_from(index).ok()?)?.to_vec(),
                path: self.path(index)?,
            });
        }
        Some(CustodyProof {
            round,
            leaves: proofs,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn data_splits_into_leaves_of_at_most_the_limit() {
        assert_eq!(split_leaves(b"").len(), 1);
        let data = vec![7u8; MAX_LEAF_BYTES * 2 + 5];
        let leaves = split_leaves(&data);
        assert_eq!(leaves.len(), 3);
        assert_eq!(leaves[2].len(), 5);
        let tree = DataTree::of_data(&data);
        assert_eq!(tree.size(), 3);
        let hashes: Vec<Hash32> = leaves.iter().map(|l| merkle::leaf_hash(l)).collect();
        assert_eq!(tree.root(), merkle::root(&hashes));
    }

    #[test]
    fn challenges_differ_by_round_and_position_and_stay_in_range() {
        let seed = first_seed(&Hash32::ZERO, &ObjectId::derive(b"deal"));
        let a = challenge_index(&seed, 0, 0, 1 << 30);
        let b = challenge_index(&seed, 1, 0, 1 << 30);
        let c = challenge_index(&seed, 0, 1, 1 << 30);
        assert!(a != b || a != c);
        assert_eq!(challenge_index(&seed, 5, 3, 1), 0);
        let next = next_seed(&Hash32::ZERO, &ObjectId::derive(b"deal"), &seed);
        assert_ne!(next, seed);
    }

    #[test]
    fn rounds_count_from_the_start() {
        assert_eq!(current_round(Pulse(10), 8, Pulse(9)), None);
        assert_eq!(current_round(Pulse(10), 8, Pulse(10)), Some(0));
        assert_eq!(current_round(Pulse(10), 8, Pulse(17)), Some(0));
        assert_eq!(current_round(Pulse(10), 8, Pulse(18)), Some(1));
    }

    proptest! {
        /// The kept levels give the same root and paths as the RFC 9162
        /// functions, and the paths verify.
        #[test]
        fn levels_match_the_rfc_tree(n in 1usize..300, pick in any::<u64>()) {
            let leaves: Vec<Hash32> = (0..n as u64).map(|i| merkle::leaf_hash(&i.to_be_bytes())).collect();
            let tree = DataTree::from_leaf_hashes(leaves.clone());
            prop_assert_eq!(tree.root(), merkle::root(&leaves));
            let index = pick % n as u64;
            let path = tree.path(index).unwrap();
            prop_assert_eq!(&path, &merkle::inclusion_proof(&leaves, index as usize).unwrap());
            prop_assert!(merkle::verify_inclusion(&tree.root(), n as u64, index, &leaves[index as usize], &path).is_ok());
            prop_assert!(tree.path(n as u64).is_none());
        }
    }

    #[test]
    fn a_provider_answers_from_its_copy() {
        let data: Vec<u8> = (0..20_000u32).flat_map(|i| i.to_le_bytes()).collect();
        let tree = DataTree::of_data(&data);
        let custody = Custody {
            deal: ObjectId::derive(b"deal"),
            root: tree.root(),
            size: tree.size(),
            challenges: 4,
            start: Pulse(1),
            round_length: 8,
            rounds: 3,
            seed: first_seed(&Hash32::ZERO, &ObjectId::derive(b"deal")),
            next: 0,
            proven: 0,
        };
        let proof = tree.answer(&data, &custody, 0).unwrap();
        assert_eq!(proof.leaves.len(), 4);
        for (k, leaf) in proof.leaves.iter().enumerate() {
            let index = challenge_index(&custody.seed, 0, k as u8, custody.size);
            assert!(merkle::verify_inclusion(
                &custody.root,
                custody.size,
                index,
                &merkle::leaf_hash(&leaf.data),
                &leaf.path
            )
            .is_ok());
        }
    }
}
