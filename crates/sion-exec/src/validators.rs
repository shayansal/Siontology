//! Validators, stake and epochs, and the forced-inclusion obligations that
//! hold shard committees to account.
//!
//! **Stake.** A validator joins by staking the stake asset (a stablecoin
//! at first, a staking token later) with a proof that it holds its keys
//! ([`crate::TxKind::RegisterValidator`]) and enters the active set at the
//! next epoch boundary. It leaves with `Unstake`, stays bonded for an
//! unbonding period, then withdraws. Equivocation (two different consensus
//! headers for one round, or two lane approvals spending one object version
//! while both transactions are live) leaves signed evidence, and anyone who
//! reports it is paid a share of the offender's slashed stake. At every
//! epoch boundary, a share of the fees the treasury collected during the
//! epoch is paid to that epoch's validators in proportion to their weight;
//! nothing else the treasury receives is paid out.
//!
//! **Committees.** An epoch's consensus committee is its active set, weighted
//! by stake; each shard's lane committee is sampled from the active set with
//! the randomness the previous epoch ended with. All of it lives in the
//! state, so every committee change follows these rules and is covered by
//! the World Proof: the chain of signers is valid from genesis.
//!
//! **Forced inclusion.** An intent posted straight to consensus becomes an
//! obligation of its sender's shard. By its deadline the intent must have
//! stopped being executable: it executed, its inputs were spent, it
//! expired, or its sender rotated keys. If it can still execute, the shard
//! has censored it, and its committee is frozen out: from the next Pulse the
//! shard's certificates come from a committee resampled, as far as the
//! validator set allows, from validators outside the one that failed (at
//! most once per shard per forced-inclusion window).

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sion_consensus::{
    sample_committee, ApprovalProof, Committee, ConsensusError, SignatureVerifier, SignedHeader,
};
use sion_core::hash::{hash_parts, Hash32};
use sion_core::ids::{AccountId, AssetId, Epoch, Pulse, ShardId, TxDigest, ValidatorId};
use sion_core::Amount;
use sion_crypto::{verify_validator, PublicKey};

use crate::object::ObjectRef;
use crate::state::ChainConfig;
use crate::tx::Transaction;

/// The largest consensus weight one validator can have (so that the sum
/// over any active set fits in 64 bits).
pub const MAX_WEIGHT: u64 = 1_000_000_000_000;

/// A validator's consensus weight: its stake in whole units of the stake
/// asset (10⁶ micro-units), at least 1 and at most [`MAX_WEIGHT`].
pub fn weight(stake: Amount) -> u64 {
    u64::try_from(stake.0 / 1_000_000)
        .unwrap_or(u64::MAX)
        .clamp(1, MAX_WEIGHT)
}

/// `floor(amount × permille / 1000)` without overflow.
pub fn permille_of(amount: Amount, permille: u16) -> Amount {
    let p = u128::from(permille.min(1000));
    Amount((amount.0 / 1000) * p + (amount.0 % 1000) * p / 1000)
}

/// `floor(amount × part / whole)` without overflow, for `part ≤ whole`.
pub fn share_of(amount: Amount, part: u64, whole: u64) -> Amount {
    if whole == 0 {
        return Amount::ZERO;
    }
    let (part, whole) = (u128::from(part.min(whole)), u128::from(whole));
    Amount((amount.0 / whole) * part + (amount.0 % whole) * part / whole)
}

/// Parameters fixed at genesis.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct ValidatorParams {
    /// The asset validators stake: a stablecoin in the first phases, a
    /// dedicated staking token later. Rewards are always paid in USD, from
    /// fees.
    pub stake_asset: AssetId,
    /// Pulses per epoch (at least 1).
    pub epoch_length: u64,
    /// The least stake a validator may register with.
    pub min_stake: Amount,
    /// Pulses between leaving the active set and being able to withdraw.
    pub unbonding: u64,
    /// Seats in each shard's lane committee (at least 1).
    pub seats: u16,
    /// The most validators in the active set (by stake, at least 1).
    pub max_active: u16,
    /// Share of the fees collected during an epoch paid to the epoch's
    /// validators, in thousandths.
    pub reward_permille: u16,
    /// Share of slashed stake paid to whoever reported it, in thousandths;
    /// the rest goes to the treasury.
    pub reporter_permille: u16,
}

impl ValidatorParams {
    /// Devnet defaults: epochs effectively never end (a devnet's validator
    /// set is fixed unless it is configured otherwise); 70 % of an epoch's
    /// fees go to its validators and 5 % of a slashed stake to the reporter,
    /// as on the node's devnet (`sion-node`'s `devnet.rs`).
    pub fn devnet() -> Self {
        ValidatorParams {
            stake_asset: AssetId::usd(),
            epoch_length: u64::MAX / 2,
            min_stake: Amount::units(10_000),
            unbonding: 100,
            seats: 4,
            max_active: 100,
            reward_permille: 700,
            reporter_permille: 50,
        }
    }

    /// Whether the parameters are usable.
    pub fn check(&self) -> Result<(), String> {
        if self.epoch_length == 0 || self.seats == 0 || self.max_active == 0 {
            return Err("epoch length, seats and the active-set size must be positive".into());
        }
        if self.reward_permille > 1000 || self.reporter_permille > 1000 {
            return Err("shares are thousandths, at most 1000".into());
        }
        Ok(())
    }
}

/// Where a validator stands.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub enum ValidatorStatus {
    /// Staked; joins the active set at the next epoch boundary.
    Joining,
    /// Eligible for the active set.
    Active,
    /// Leaves at the next epoch boundary.
    Leaving,
    /// Out; its stake can be withdrawn from `withdrawable_at`.
    Exited {
        /// First Pulse at which the stake can be withdrawn.
        withdrawable_at: Pulse,
    },
    /// Slashed for equivocation; its stake is gone.
    Slashed {
        /// When.
        at: Pulse,
    },
    /// Its stake was withdrawn.
    Withdrawn,
}

/// One registered validator.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct ValidatorRecord {
    /// Its identifier (derived from its keys, except for genesis validators
    /// of a devnet that signs with the insecure simulation scheme).
    pub id: ValidatorId,
    /// ML-DSA-44 public key.
    pub lattice: PublicKey,
    /// SLH-DSA public key.
    pub hash: PublicKey,
    /// The account that staked, and receives rewards and withdrawals.
    pub operator: AccountId,
    /// Stake held, in the stake asset.
    pub stake: Amount,
    /// Status.
    pub status: ValidatorStatus,
    /// Tip of its beacon hash chain, for the randomness beacon.
    pub beacon_tip: Hash32,
    /// Rewards accrued and not yet claimed, in USD.
    pub rewards: Amount,
    /// When it registered.
    pub registered_at: Pulse,
}

/// A shard whose committee was replaced after a forced-inclusion violation.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct ShardRotation {
    /// Rotations of this shard in the current epoch.
    pub count: u32,
    /// The committee now serving, sorted.
    pub members: Vec<ValidatorId>,
    /// The Pulse of the latest rotation.
    pub at: Pulse,
}

/// The validator set: a shared protocol object.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct ValidatorSet {
    /// Parameters.
    pub params: ValidatorParams,
    /// The current epoch (0 from genesis).
    pub epoch: u64,
    /// The first Pulse of the current epoch.
    pub epoch_start: Pulse,
    /// The randomness this epoch's committees are sampled with.
    pub seed: Hash32,
    /// The active set, (validator, weight), sorted by validator.
    pub active: Vec<(ValidatorId, u64)>,
    /// Every validator ever registered.
    pub validators: BTreeMap<ValidatorId, ValidatorRecord>,
    /// Shards whose committee was rotated this epoch, by shard number.
    pub rotations: BTreeMap<u16, ShardRotation>,
    /// The treasury's running total of USD fees when the epoch began:
    /// the epoch's rewards are a share of what it gains.
    pub fees_mark: Amount,
}

impl ValidatorSet {
    /// The last Pulse of the current epoch.
    pub fn epoch_end(&self) -> Pulse {
        Pulse(
            self.epoch_start
                .0
                .saturating_add(self.params.epoch_length.max(1) - 1),
        )
    }

    /// The consensus committee (it certifies Pulse headers) of the current
    /// epoch on the chain with identity `chain`.
    pub fn consensus_committee(&self, chain: Hash32) -> Result<Committee, ConsensusError> {
        Committee::new(Epoch(self.epoch), ShardId(0), self.active.clone())
            .map(|committee| committee.on_chain(chain))
    }

    /// The members of `shard`'s lane committee, sorted.
    pub fn lane_members(&self, shard: ShardId) -> Result<Vec<ValidatorId>, ConsensusError> {
        Ok(match self.rotations.get(&shard.0) {
            Some(rotation) => rotation.members.clone(),
            None => sample_committee(
                &self.active,
                usize::from(self.params.seats),
                &self.seed,
                Epoch(self.epoch),
                shard,
            )?
            .members()
            .iter()
            .map(|(v, _)| *v)
            .collect(),
        })
    }

    /// The rotations of `shard` in the current epoch.
    pub fn rotation(&self, shard: ShardId) -> u32 {
        self.rotations.get(&shard.0).map(|r| r.count).unwrap_or(0)
    }

    /// `shard`'s lane committee (one seat per member) on chain `chain`, with
    /// committee epoch [`lane_epoch`].
    pub fn lane_committee(
        &self,
        chain: Hash32,
        shard: ShardId,
    ) -> Result<Committee, ConsensusError> {
        Committee::equal_weight(
            lane_epoch(self.epoch, self.rotation(shard)),
            shard,
            self.lane_members(shard)?,
        )
        .map(|committee| committee.on_chain(chain))
    }

    /// Every shard's lane committee.
    pub fn lane_committees(
        &self,
        chain: Hash32,
        shards: u16,
    ) -> Result<BTreeMap<ShardId, Committee>, ConsensusError> {
        (0..shards.max(1))
            .map(|s| Ok((ShardId(s), self.lane_committee(chain, ShardId(s))?)))
            .collect()
    }

    /// Verifies signatures by registered validators with their keys.
    pub fn verifier(&self) -> RegisteredKeys<'_> {
        RegisteredKeys(self)
    }
}

/// Signature verification against the keys registered in a [`ValidatorSet`].
#[derive(Clone, Copy, Debug)]
pub struct RegisteredKeys<'a>(&'a ValidatorSet);

impl SignatureVerifier for RegisteredKeys<'_> {
    fn verify(&self, validator: &ValidatorId, message: &[u8], signature: &[u8]) -> bool {
        match self.0.validators.get(validator) {
            Some(record) => verify_validator(&record.lattice, &record.hash, message, signature),
            None => false,
        }
    }
}

/// The committee epoch of a lane committee: the validator-set epoch in the
/// high 32 bits and the shard's rotations in it in the low 32 bits. Lane
/// approvals name it, so an approval made before a rotation or an epoch
/// change never counts after it.
pub fn lane_epoch(epoch: u64, rotation: u32) -> Epoch {
    Epoch((epoch.min(u64::from(u32::MAX)) << 32) | u64::from(rotation))
}

/// What a validator signs to prove it holds its keys when it stakes: bound
/// to the chain configuration, the operator and the validator.
pub fn possession_digest(config: &ChainConfig, operator: &AccountId, id: &ValidatorId) -> Hash32 {
    hash_parts(
        "sion/validators/possession",
        &[&config.digest().0, &operator.0 .0, &id.0 .0],
    )
}

/// The seed of a shard's `count`-th rotation.
pub fn rotation_seed(beacon: &Hash32, shard: ShardId, count: u32) -> Hash32 {
    hash_parts(
        "sion/validators/rotation",
        &[&beacon.0, &shard.0.to_be_bytes(), &count.to_be_bytes()],
    )
}

/// One lane approval offered as evidence: the approved transaction's body
/// (its digest and inputs are what was approved) and the signed approval.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct EvidenceApproval {
    /// The transaction body.
    pub tx: Transaction,
    /// The validator's approval of it.
    pub proof: ApprovalProof,
}

/// Signed evidence that a validator equivocated.
///
/// Transactions carry it boxed (`TxKind::ReportEquivocation`), so the size
/// of the header variant costs nothing elsewhere.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub enum Equivocation {
    /// One author signed two different consensus headers for the same
    /// chain, epoch, shard and round.
    Headers {
        /// One header.
        first: SignedHeader,
        /// The other.
        second: SignedHeader,
    },
    /// One validator approved two different transactions that spend the
    /// same object version. Honest validators approve a version again only
    /// after the first transaction can no longer execute, so this counts
    /// only while both transactions are unexpired.
    Approvals {
        /// One approval.
        first: Box<EvidenceApproval>,
        /// The other.
        second: Box<EvidenceApproval>,
    },
}

fn lane_refs(inputs: &[ObjectRef]) -> Vec<sion_consensus::ObjectRef> {
    inputs
        .iter()
        .map(|r| sion_consensus::ObjectRef {
            id: r.id,
            version: r.version,
        })
        .collect()
}

impl Equivocation {
    /// The validator this evidence incriminates, if it holds at `pulse`:
    /// both messages are signed with the keys `set` registers for it, and
    /// they conflict.
    pub fn offender(&self, set: &ValidatorSet, pulse: Pulse) -> Result<ValidatorId, String> {
        let keys = set.verifier();
        match self {
            Equivocation::Headers { first, second } => {
                let (a, b) = (&first.header, &second.header);
                if a.author != b.author
                    || a.chain != b.chain
                    || a.epoch != b.epoch
                    || a.shard != b.shard
                    || a.round != b.round
                {
                    return Err("the headers are not for the same author and round".into());
                }
                if a.digest() == b.digest() {
                    return Err("the headers are the same".into());
                }
                if !first.verify_signature(&keys) || !second.verify_signature(&keys) {
                    return Err("a header signature does not verify".into());
                }
                Ok(a.author)
            }
            Equivocation::Approvals { first, second } => {
                let (a, b) = (&first.proof.batch, &second.proof.batch);
                if a.validator != b.validator
                    || a.chain != b.chain
                    || a.epoch != b.epoch
                    || a.shard != b.shard
                {
                    return Err("the approvals are not by the same validator".into());
                }
                let (da, db) = (first.tx.digest(), second.tx.digest());
                if da == db {
                    return Err("the approvals are of the same transaction".into());
                }
                if pulse > first.tx.expires_at || pulse > second.tx.expires_at {
                    return Err("an approved transaction has expired".into());
                }
                if !first
                    .tx
                    .inputs
                    .iter()
                    .any(|input| second.tx.inputs.contains(input))
                {
                    return Err("the transactions spend no common object version".into());
                }
                first
                    .proof
                    .verify_signed(&da, &lane_refs(&first.tx.inputs), &keys)
                    .map_err(|e| e.to_string())?;
                second
                    .proof
                    .verify_signed(&db, &lane_refs(&second.tx.inputs), &keys)
                    .map_err(|e| e.to_string())?;
                Ok(a.validator)
            }
        }
    }
}

/// Forced-inclusion parameters, fixed at genesis.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct ForcedParams {
    /// Pulses a shard has to make a posted intent unexecutable (at least 1).
    pub window: u64,
    /// The most obligations pending at once; later intents are dismissed.
    pub max_pending: u32,
}

impl ForcedParams {
    /// Devnet defaults.
    pub fn devnet() -> Self {
        ForcedParams {
            window: 8,
            max_pending: 4_096,
        }
    }
}

/// One forced intent its shard still owes.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Obligation {
    /// The shard that owes it (its sender's).
    pub shard: u16,
    /// The sender.
    pub sender: AccountId,
    /// The intent's inputs, at the versions it spends.
    pub inputs: Vec<ObjectRef>,
    /// The intent's expiry.
    pub expires_at: Pulse,
    /// The sender account's policy rotations when the intent was posted.
    pub rotations: u32,
    /// The Pulse it was posted in.
    pub posted_at: Pulse,
    /// The Pulse at whose end it is checked.
    pub deadline: Pulse,
    /// Deadlines it has missed.
    pub missed: u32,
}

/// Forced-inclusion obligations: a shared protocol object.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct ForcedObligations {
    /// Parameters.
    pub params: ForcedParams,
    /// Pending obligations by intent digest.
    pub pending: BTreeMap<TxDigest, Obligation>,
}

impl ForcedObligations {
    /// Pending obligations that have missed at least one deadline.
    pub fn overdue(&self) -> impl Iterator<Item = (&TxDigest, &Obligation)> {
        self.pending.iter().filter(|(_, o)| o.missed > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_shares_never_overflow() {
        assert_eq!(permille_of(Amount(1_999), 500), Amount(999));
        assert_eq!(permille_of(Amount(u128::MAX), 1000), Amount(u128::MAX));
        assert_eq!(permille_of(Amount(u128::MAX), 0), Amount::ZERO);
        assert_eq!(share_of(Amount(10), 1, 3), Amount(3));
        assert_eq!(
            share_of(Amount(u128::MAX), u64::MAX, u64::MAX),
            Amount(u128::MAX)
        );
        assert_eq!(share_of(Amount(100), 5, 0), Amount::ZERO);
        let parts = [3u64, 5, 7];
        let total: u128 = parts
            .iter()
            .map(|p| share_of(Amount(1_000_003), *p, 15).0)
            .sum();
        assert!(total <= 1_000_003, "shares never exceed the amount");
        assert_eq!(weight(Amount(0)), 1);
        assert_eq!(weight(Amount::units(25)), 25);
        assert_eq!(weight(Amount(u128::MAX)), MAX_WEIGHT);
    }
}
