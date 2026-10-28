//! Payment streams (specification §5.31 to §5.34).
//!
//! A payer escrows funds for one payee and pays it off the ledger, in steps
//! of a fixed unit, by revealing the elements of a hash chain it keeps
//! secret. Checking a revealed element against the previous one costs one
//! hash; the payee settles many payments in one transaction, which checks
//! one hash per step. Hashes are the only cryptography a payment needs, so
//! payments stay post-quantum without a signature each.
//!
//! The chain: with `step(x) = H("sion/stream/step", x)`, the payer picks a
//! secret seed `w_n`, computes `w_{i−1} = step(w_i)` down to `w_0`, and
//! opens the stream with `anchor = w_0`. Revealing `w_i` pays `i` steps in
//! total. The stream object records the settled `position` and its element
//! `head`, so any element at or below a settled position can be checked by
//! anyone: hash down from the head.
//!
//! **Checkpoints** (spec v1.7). Walking from a settled head to a new
//! position costs one hash per step, so settling many payments costs the
//! ledger as much as checking them did. A payer may also commit, at open, to
//! a Merkle tree over every `spacing`-th chain element: leaf `j` is
//! [`checkpoint_leaf`]`(j, w_{j·spacing})`. A payee who holds the path for
//! leaf `j` then settles any position in `[j·spacing, (j+1)·spacing)` with
//! a [`Jump`]: it walks to the checkpoint and proves the leaf, at most
//! `spacing + 1 + 20` hashes however many payments the settlement covers.
//! The leaves reveal nothing about the chain (they are hashes of its
//! elements), so the payer can hand paths out freely.
//!
//! This module defines the stream object and the constants of the
//! specification, and helpers for payers and payees off the ledger
//! ([`step`], [`walk`], [`checkpoint_leaf`], [`verify_jump`],
//! [`PayerChain`]). The rules that change the state are in
//! [`crate::execute`].

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sion_core::hash::{hash_domain, Hash32};
use sion_core::ids::{AccountId, AssetId, Pulse};
use sion_core::{merkle, Amount};

/// The longest hash chain a stream may commit to (2³²).
pub const MAX_STREAM_LENGTH: u64 = 1 << 32;

/// The most steps one settlement may check.
pub const MAX_SETTLE_STEPS: u64 = 65_536;

/// The fee per settled step, on top of the minimum fee: 2 micro-units of
/// USD, about what checking one hash costs a validator, measured against
/// the minimum fee that covers checking one transaction signature.
pub const STREAM_STEP_FEE: Amount = Amount(2);

/// The hash domain of a chain step.
pub const STEP_DOMAIN: &str = "sion/stream/step";

/// The most checkpoints a stream may commit to (2²⁰), so a checkpoint's
/// Merkle path has at most [`MAX_CHECKPOINT_DEPTH`] hashes.
pub const MAX_CHECKPOINTS: u64 = 1 << 20;

/// The longest Merkle path a jump may carry.
pub const MAX_CHECKPOINT_DEPTH: usize = 20;

/// The tag that starts a checkpoint leaf's data.
pub const CHECKPOINT_TAG: &[u8] = b"sion/stream/checkpoint";

/// A payer's commitment to every `spacing`-th element of its chain.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub struct Checkpoints {
    /// Steps between checkpoints.
    pub spacing: u64,
    /// The Merkle tree hash of the leaves `checkpoint_leaf(j, w_{j·spacing})`
    /// for `j = 0, …, length / spacing`.
    pub root: Hash32,
}

/// A settlement's proof that its element is genuine without walking to the
/// head: the checkpoint below it and that checkpoint's Merkle path.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Jump {
    /// The checkpoint, `j`: the element is at a position in
    /// `[j·spacing, (j+1)·spacing)`.
    pub index: u64,
    /// The Merkle path of leaf `j`, from the leaf up.
    pub path: Vec<Hash32>,
}

/// A payment stream: a shared object in its own contention domain.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Stream {
    /// Who escrowed the funds and reveals the chain.
    pub payer: AccountId,
    /// Who is paid, and the only account that may settle.
    pub payee: AccountId,
    /// The asset escrowed and paid.
    pub asset: AssetId,
    /// The value of one step.
    pub unit: Amount,
    /// How many steps the chain has.
    pub length: u64,
    /// Steps settled so far.
    pub position: u64,
    /// The chain element at `position` (the anchor when nothing is settled).
    pub head: Hash32,
    /// What is left in escrow.
    pub escrow: Amount,
    /// The last Pulse in which the payee may settle; after it, the payer may
    /// close the stream and take back what is left.
    pub until: Pulse,
    /// Whether the stream is still open.
    pub open: bool,
    /// Steps between the checkpoints the payer committed to (0: none).
    pub spacing: u64,
    /// The checkpoints' Merkle root ([`Hash32::ZERO`] without checkpoints).
    pub checkpoints: Hash32,
}

impl Stream {
    /// How many checkpoint leaves the stream commits to (0 without
    /// checkpoints).
    pub fn checkpoint_count(&self) -> u64 {
        match self.spacing {
            0 => 0,
            spacing => self.length / spacing + 1,
        }
    }
}

/// One chain step: `H("sion/stream/step", x)`.
pub fn step(x: &Hash32) -> Hash32 {
    hash_domain(STEP_DOMAIN, &x.0)
}

/// `steps` applications of [`step`] to `x`.
pub fn walk(x: &Hash32, steps: u64) -> Hash32 {
    let mut value = *x;
    for _ in 0..steps {
        value = step(&value);
    }
    value
}

/// Whether `value` is the element `steps` positions above `head` in a chain
/// (a payee's check of a payment, and a third party's check of an item).
pub fn extends(head: &Hash32, value: &Hash32, steps: u64) -> bool {
    walk(value, steps) == *head
}

/// Checkpoint leaf `index`: the Merkle leaf hash of `CHECKPOINT_TAG ‖
/// be64(index) ‖ element`.
pub fn checkpoint_leaf(index: u64, element: &Hash32) -> Hash32 {
    let mut data = Vec::with_capacity(CHECKPOINT_TAG.len() + 8 + 32);
    data.extend_from_slice(CHECKPOINT_TAG);
    data.extend_from_slice(&index.to_be_bytes());
    data.extend_from_slice(&element.0);
    merkle::leaf_hash(&data)
}

/// The hashes a jump to `position` costs: the walk down to the checkpoint,
/// the leaf, and the path. `None` if the jump cannot apply to `stream` at
/// `position`.
pub fn jump_hashes(stream: &Stream, position: u64, jump: &Jump) -> Option<u64> {
    if stream.spacing == 0 || jump.path.len() > MAX_CHECKPOINT_DEPTH {
        return None;
    }
    let base = jump.index.checked_mul(stream.spacing)?;
    if position < base || position - base >= stream.spacing {
        return None;
    }
    Some(position - base + 1 + jump.path.len() as u64)
}

/// Whether `value` is the element at `position` of `stream`'s chain, by
/// `jump`: walk to the checkpoint below it and prove that checkpoint's leaf.
pub fn verify_jump(stream: &Stream, position: u64, value: &Hash32, jump: &Jump) -> bool {
    if jump_hashes(stream, position, jump).is_none() {
        return false;
    }
    let base = jump.index * stream.spacing;
    let checkpoint = walk(value, position - base);
    merkle::verify_inclusion(
        &stream.checkpoints,
        stream.checkpoint_count(),
        jump.index,
        &checkpoint_leaf(jump.index, &checkpoint),
        &jump.path,
    )
    .is_ok()
}

/// A payer's hash chain: derives the anchor and serves elements in
/// increasing positions, keeping one element every √length positions so
/// that a reveal costs at most √length hashes, and one on average when
/// elements are revealed in order.
#[derive(Clone, Debug)]
pub struct PayerChain {
    length: u64,
    /// Elements at multiples of a spacing of about √length, and at `length`.
    marks: BTreeMap<u64, Hash32>,
    /// Elements from the last mark used, by position.
    window: BTreeMap<u64, Hash32>,
    /// The checkpoint spacing and leaves, if the chain has checkpoints.
    checkpoints: Option<(u64, Vec<Hash32>)>,
}

impl PayerChain {
    /// The chain of `length` steps whose top element is `seed`. Costs
    /// `length` hashes once.
    pub fn new(seed: Hash32, length: u64) -> PayerChain {
        Self::build(seed, length, 0)
    }

    /// Like [`PayerChain::new`], also committing to every `spacing`-th
    /// element (spec v1.7): the stream opens with [`PayerChain::checkpoints`]
    /// and the payer hands out [`PayerChain::jump`]s. Costs `length / spacing
    /// + 1` more hashes, and as many leaves in memory.
    ///
    /// # Panics
    ///
    /// If `spacing` is 0 or more than `length`, or the chain would have more
    /// than [`MAX_CHECKPOINTS`] checkpoints.
    pub fn with_checkpoints(seed: Hash32, length: u64, spacing: u64) -> PayerChain {
        assert!(
            spacing >= 1 && spacing <= length && length / spacing < MAX_CHECKPOINTS,
            "a checkpoint spacing between 1 and the length, with at most 2^20 checkpoints"
        );
        Self::build(seed, length, spacing)
    }

    fn build(seed: Hash32, length: u64, checkpoint_spacing: u64) -> PayerChain {
        let spacing = (length as f64).sqrt().ceil().max(1.0) as u64;
        let mut marks = BTreeMap::new();
        let count = match checkpoint_spacing {
            0 => 0,
            c => length / c + 1,
        };
        let mut leaves = vec![Hash32::ZERO; count as usize];
        let mut value = seed;
        let mut position = length;
        marks.insert(position, value);
        loop {
            if checkpoint_spacing > 0 && position % checkpoint_spacing == 0 {
                let index = position / checkpoint_spacing;
                leaves[index as usize] = checkpoint_leaf(index, &value);
            }
            if position == 0 {
                break;
            }
            value = step(&value);
            position -= 1;
            if position % spacing == 0 {
                marks.insert(position, value);
            }
        }
        PayerChain {
            length,
            marks,
            window: BTreeMap::new(),
            checkpoints: (checkpoint_spacing > 0).then_some((checkpoint_spacing, leaves)),
        }
    }

    /// The checkpoints to open the stream with, if the chain has them.
    pub fn checkpoints(&self) -> Option<Checkpoints> {
        self.checkpoints
            .as_ref()
            .map(|(spacing, leaves)| Checkpoints {
                spacing: *spacing,
                root: merkle::root(leaves),
            })
    }

    /// The jump for checkpoint `index`: what a payee needs to settle any
    /// position from `index · spacing` up to the next checkpoint without
    /// walking to its head. Costs about two hashes per checkpoint.
    pub fn jump(&self, index: u64) -> Option<Jump> {
        let (_, leaves) = self.checkpoints.as_ref()?;
        let path = merkle::inclusion_proof(leaves, usize::try_from(index).ok()?).ok()?;
        Some(Jump { index, path })
    }

    /// The checkpoint spacing (0 without checkpoints).
    pub fn spacing(&self) -> u64 {
        self.checkpoints.as_ref().map(|(s, _)| *s).unwrap_or(0)
    }

    /// The chain's length.
    pub fn length(&self) -> u64 {
        self.length
    }

    /// The anchor `w_0`, which opens the stream.
    pub fn anchor(&self) -> Hash32 {
        self.marks[&0]
    }

    /// The element at `position` (at most the length).
    pub fn element(&mut self, position: u64) -> Option<Hash32> {
        if position > self.length {
            return None;
        }
        if let Some(value) = self.window.get(&position) {
            return Some(*value);
        }
        // Recompute the segment between the nearest marks around
        // `position`: at most `spacing` hashes, after which reveals in
        // increasing order up to the upper mark cost nothing.
        let (&top, &value) = self.marks.range(position..).next()?;
        let bottom = self
            .marks
            .range(..top)
            .next_back()
            .map(|(&p, _)| p)
            .unwrap_or(0);
        self.window.clear();
        let (mut value, mut at) = (value, top);
        self.window.insert(at, value);
        while at > bottom {
            value = step(&value);
            at -= 1;
            self.window.insert(at, value);
        }
        self.window.get(&position).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chain_reveals_elements_that_hash_down_to_the_anchor() {
        let seed = Hash32([7; 32]);
        let mut chain = PayerChain::new(seed, 1_000);
        let anchor = chain.anchor();
        assert_eq!(walk(&seed, 1_000), anchor);
        let mut last = anchor;
        for position in 1..=1_000 {
            let value = chain.element(position).unwrap();
            assert_eq!(step(&value), last, "position {position}");
            assert!(extends(&anchor, &value, position));
            last = value;
        }
        assert_eq!(chain.element(1_000), Some(seed));
        assert_eq!(chain.element(1_001), None);
        // Out of order works too.
        assert!(extends(&anchor, &chain.element(17).unwrap(), 17));
        assert!(extends(&anchor, &chain.element(999).unwrap(), 999));
    }

    #[test]
    fn jumps_prove_positions_without_walking_to_the_head() {
        let seed = Hash32([8; 32]);
        let mut chain = PayerChain::with_checkpoints(seed, 1_000, 64);
        let checkpoints = chain.checkpoints().unwrap();
        // The plain chain is the same chain.
        assert_eq!(chain.anchor(), PayerChain::new(seed, 1_000).anchor());
        let stream = Stream {
            payer: AccountId::derive(b"payer"),
            payee: AccountId::derive(b"payee"),
            asset: AssetId::usd(),
            unit: Amount(1),
            length: 1_000,
            position: 0,
            head: chain.anchor(),
            escrow: Amount(1_000),
            until: Pulse(10),
            open: true,
            spacing: 64,
            checkpoints: checkpoints.root,
        };
        assert_eq!(stream.checkpoint_count(), 16);
        for position in [0, 1, 63, 64, 640, 700, 703, 959, 960, 1_000] {
            let value = chain.element(position).unwrap();
            let jump = chain.jump(position / 64).unwrap();
            assert!(verify_jump(&stream, position, &value, &jump), "{position}");
            assert_eq!(
                jump_hashes(&stream, position, &jump),
                Some(position % 64 + 1 + jump.path.len() as u64)
            );
            // The wrong checkpoint, a wrong element or a wrong path fail.
            if position >= 64 {
                let lower = chain.jump(position / 64 - 1).unwrap();
                assert!(!verify_jump(&stream, position, &value, &lower));
            }
            assert!(!verify_jump(&stream, position, &Hash32([1; 32]), &jump));
            if let Some(first) = jump.path.first() {
                let mut bent = jump.clone();
                bent.path[0] = step(first);
                assert!(!verify_jump(&stream, position, &value, &bent));
            }
        }
        // Without checkpoints there is nothing to jump to.
        let plain = Stream {
            spacing: 0,
            checkpoints: Hash32::ZERO,
            ..stream
        };
        let value = chain.element(70).unwrap();
        assert!(!verify_jump(&plain, 70, &value, &chain.jump(1).unwrap()));
    }

    #[test]
    fn short_chains_work() {
        for length in 1..=5 {
            let mut chain = PayerChain::new(Hash32([1; 32]), length);
            for position in 0..=length {
                let value = chain.element(position).unwrap();
                assert!(extends(&chain.anchor(), &value, position));
            }
        }
    }
}
