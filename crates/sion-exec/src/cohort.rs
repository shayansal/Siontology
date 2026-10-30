//! Cohorts: up to a million subagents committed in one transaction (spec
//! v1.14, §5.55 and §5.56).
//!
//! A [`Cohort`] commits to its members as the root of a Merkle **sum**
//! tree: every node carries the sum of its leaves' bursts and rates as well
//! as a hash, so the root commits to the totals the parent's bucket gives
//! up when the cohort is created. A member costs nothing on the ledger
//! until it is **materialized**: the parent shows its leaf and the path to
//! the root, and the member's charter and allotment are created at its
//! lineage id. A proof whose sums do not add up to what the root commits to
//! does not verify, so the members that can ever be materialized never
//! carry more burst or rate than was carved.
//!
//! [`CohortTree`] builds the tree and its proofs, for clients.

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sion_core::encode::encode;
use sion_core::hash::{hash_domain, Hash32};
use sion_core::ids::{AccountId, ObjectId, Pulse};
use sion_core::Amount;

use crate::charter::Caveats;

/// The most members one cohort may commit to (spec §5.55): 2²⁰.
pub const MAX_COHORT: u32 = 1 << 20;

/// The most members one `MaterializeAgents` may materialize (spec §5.56).
pub const MAX_MATERIALIZE: usize = 128;

/// One member of a cohort: a leaf of its sum tree.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct CohortMember {
    /// The account that acts for it, or `None` for a keyless member.
    pub agent: Option<AccountId>,
    /// Commitment to its card.
    pub card: Hash32,
    /// The budget its allotment gets when it is materialized, from the
    /// parent's allotment.
    pub budget: Amount,
    /// Its bucket's capacity.
    pub burst: Amount,
    /// What refills its bucket each Pulse.
    pub rate: Amount,
}

/// A node of a cohort's sum tree.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub struct SumNode {
    /// The node's hash.
    pub hash: Hash32,
    /// The sum of its leaves' bursts.
    pub burst: Amount,
    /// The sum of its leaves' rates.
    pub rate: Amount,
}

/// A cohort (a shared object in its own domain, never written after it
/// is created).
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Cohort {
    /// The parent charter.
    pub parent: ObjectId,
    /// The lineage index of member 0: member `i` is
    /// `lineage_id(parent, first + i)`.
    pub first: u64,
    /// How many members.
    pub count: u32,
    /// The sum tree's root hash.
    pub root: Hash32,
    /// The sum of the members' bursts, carved from the parent's bucket.
    pub burst: Amount,
    /// The sum of the members' rates, carved from the parent's bucket.
    pub rate: Amount,
    /// Whether the members' buckets start full (the parent gave up the
    /// tokens) or empty, filling from `created_at`.
    pub full: bool,
    /// The members' caveats, each with its own burst and rate in place of
    /// these (which bound them).
    pub caveats: Caveats,
    /// Creation Pulse.
    pub created_at: Pulse,
}

impl Cohort {
    /// The root of its sum tree.
    pub fn root_node(&self) -> SumNode {
        SumNode {
            hash: self.root,
            burst: self.burst,
            rate: self.rate,
        }
    }
}

/// One member to materialize, with its proof (spec §5.56).
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Materialize {
    /// Its index in the cohort.
    pub index: u32,
    /// Its leaf.
    pub member: CohortMember,
    /// The siblings on its path, from the leaves up.
    pub siblings: Vec<SumNode>,
}

/// A member's leaf node.
pub fn member_node(member: &CohortMember) -> SumNode {
    SumNode {
        hash: hash_domain("sion/cohort/member", &encode(member)),
        burst: member.burst,
        rate: member.rate,
    }
}

/// The node at the leaves past the last member.
pub fn empty_node() -> SumNode {
    SumNode {
        hash: hash_domain("sion/cohort/empty", b""),
        burst: Amount::ZERO,
        rate: Amount::ZERO,
    }
}

/// The inner node over `left` and `right`; `None` if a sum overflows.
pub fn join(left: &SumNode, right: &SumNode) -> Option<SumNode> {
    let burst = left.burst.checked_add(right.burst).ok()?;
    let rate = left.rate.checked_add(right.rate).ok()?;
    let hash = hash_domain(
        "sion/cohort/node",
        &encode(&(
            left.hash,
            left.burst,
            left.rate,
            right.hash,
            right.burst,
            right.rate,
        )),
    );
    Some(SumNode { hash, burst, rate })
}

/// The tree's height for `count` members: `⌈log₂ count⌉`, 0 for one.
pub fn height(count: u32) -> u32 {
    if count <= 1 {
        0
    } else {
        32 - (count - 1).leading_zeros()
    }
}

/// Whether `member` is leaf `index` of the tree whose root is `root` over
/// `count` members (spec §5.55 **verify**).
pub fn verify_member(
    root: &SumNode,
    count: u32,
    index: u32,
    member: &CohortMember,
    siblings: &[SumNode],
) -> bool {
    if index >= count || siblings.len() != height(count) as usize {
        return false;
    }
    let mut node = member_node(member);
    for (level, sibling) in siblings.iter().enumerate() {
        let joined = if (index >> level) & 1 == 0 {
            join(&node, sibling)
        } else {
            join(sibling, &node)
        };
        match joined {
            Some(next) => node = next,
            None => return false,
        }
    }
    node == *root
}

/// A cohort's sum tree, built by a client: its levels from the leaves up.
#[derive(Clone, Debug)]
pub struct CohortTree {
    levels: Vec<Vec<SumNode>>,
    count: u32,
}

impl CohortTree {
    /// Builds the tree over `members`; `None` if there are none, more than
    /// [`MAX_COHORT`], or a sum overflows.
    pub fn build(members: &[CohortMember]) -> Option<Self> {
        let count = u32::try_from(members.len()).ok()?;
        if count == 0 || count > MAX_COHORT {
            return None;
        }
        let width = 1usize << height(count);
        let mut level: Vec<SumNode> = members.iter().map(member_node).collect();
        level.resize(width, empty_node());
        let mut levels = vec![level];
        while levels.last()?.len() > 1 {
            let below = levels.last()?;
            let mut above = Vec::with_capacity(below.len() / 2);
            for pair in below.chunks(2) {
                above.push(join(&pair[0], &pair[1])?);
            }
            levels.push(above);
        }
        Some(CohortTree { levels, count })
    }

    /// How many members.
    pub fn count(&self) -> u32 {
        self.count
    }

    /// The root.
    pub fn root(&self) -> SumNode {
        self.levels[self.levels.len() - 1][0]
    }

    /// The siblings on member `index`'s path, from the leaves up.
    pub fn prove(&self, index: u32) -> Option<Vec<SumNode>> {
        if index >= self.count {
            return None;
        }
        let mut position = index as usize;
        let mut siblings = Vec::with_capacity(self.levels.len() - 1);
        for level in &self.levels[..self.levels.len() - 1] {
            siblings.push(level[position ^ 1]);
            position /= 2;
        }
        Some(siblings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(n: u64) -> CohortMember {
        CohortMember {
            agent: None,
            card: hash_domain("test/card", &n.to_le_bytes()),
            budget: Amount::units(n),
            burst: Amount::units(n + 1),
            rate: Amount(u128::from(n) * 10),
        }
    }

    #[test]
    fn heights_round_up() {
        assert_eq!(height(1), 0);
        assert_eq!(height(2), 1);
        assert_eq!(height(3), 2);
        assert_eq!(height(4), 2);
        assert_eq!(height(5), 3);
        assert_eq!(height(MAX_COHORT), 20);
    }

    #[test]
    fn every_member_verifies_and_the_root_carries_the_sums() {
        for count in [1u64, 2, 3, 5, 8, 13] {
            let members: Vec<_> = (0..count).map(member).collect();
            let tree = CohortTree::build(&members).unwrap();
            let root = tree.root();
            let bursts: u128 = members.iter().map(|m| m.burst.0).sum();
            let rates: u128 = members.iter().map(|m| m.rate.0).sum();
            assert_eq!((root.burst.0, root.rate.0), (bursts, rates));
            for (i, m) in members.iter().enumerate() {
                let proof = tree.prove(i as u32).unwrap();
                assert!(verify_member(&root, count as u32, i as u32, m, &proof));
                // Not at another index, nor with another leaf.
                let other = (i as u32 + 1) % count as u32;
                if other != i as u32 {
                    assert!(!verify_member(&root, count as u32, other, m, &proof));
                }
                let mut richer = m.clone();
                richer.burst = Amount(richer.burst.0 + 1);
                assert!(!verify_member(
                    &root,
                    count as u32,
                    i as u32,
                    &richer,
                    &proof
                ));
            }
            assert!(tree.prove(count as u32).is_none());
        }
    }

    #[test]
    fn a_proof_cannot_claim_more_than_the_root_commits_to() {
        let members: Vec<_> = (0..4).map(member).collect();
        let tree = CohortTree::build(&members).unwrap();
        let root = tree.root();
        let mut proof = tree.prove(0).unwrap();
        // A sibling that understates its sums changes the hash above it.
        proof[1].burst = Amount::ZERO;
        assert!(!verify_member(&root, 4, 0, &members[0], &proof));
        // Past the members, the empty leaves cannot be shown as members.
        let tree = CohortTree::build(&members[..3]).unwrap();
        let proof = tree.prove(2).unwrap();
        assert!(!verify_member(&tree.root(), 3, 3, &members[2], &proof));
    }

    #[test]
    fn overflowing_sums_build_no_tree_and_verify_nothing() {
        let mut huge = member(1);
        huge.burst = Amount(u128::MAX);
        assert!(CohortTree::build(&[huge.clone(), member(2)]).is_none());
        let node = member_node(&huge);
        assert!(join(&node, &node).is_none());
    }
}
