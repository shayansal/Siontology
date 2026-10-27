//! Global state, configuration and the world commitment.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sion_assurance::Registry;
use sion_core::encode::{encode, Canonical};
use sion_core::hash::{hash_domain, Hash32};
use sion_core::ids::{AccountId, AssetId, ClaimId, ObjectId, Pulse};
use sion_core::merkle::AppendLog;
use sion_core::smt::SparseMerkleTree;
use sion_core::Amount;

use crate::assurance::AssuranceDomain;
use crate::claim::StoredClaim;
use crate::error::ExecError;
use crate::object::{well_known, Object, ObjectData, Treasury};
use crate::store::ObjectStore;

/// Chain parameters (part of the world commitment).
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct ChainConfig {
    /// Chain identifier.
    pub chain_id: String,
    /// Number of shards (certification and ordering committees).
    pub shard_count: u16,
    /// Minimum fee per transaction, in USD.
    pub base_fee: Amount,
    /// Fee per KiB of published transaction body (signatures are verified
    /// and proven, never published, so they are not charged per byte).
    pub fee_per_kib: Amount,
    /// The devnet faucet account, if any.
    pub faucet: Option<AccountId>,
    /// Longest claim validity window, in Pulses.
    pub max_claim_window: u64,
    /// Largest formula (nodes, before unfolding claim references).
    pub max_formula_size: usize,
    /// Largest claim payload in bytes.
    pub max_payload_bytes: usize,
    /// Share of released slashing pools paid to the successful challenger,
    /// in thousandths; the rest goes to the treasury.
    pub challenger_share_permille: u16,
}

impl ChainConfig {
    /// Devnet defaults.
    pub fn devnet(faucet: Option<AccountId>) -> Self {
        ChainConfig {
            chain_id: "sion-devnet-1".into(),
            shard_count: 2,
            base_fee: Amount(1_000),
            fee_per_kib: Amount(100),
            faucet,
            max_claim_window: 1_000_000,
            max_formula_size: 256,
            max_payload_bytes: 64 * 1024,
            challenger_share_permille: 500,
        }
    }

    /// The minimum fee for a transaction body of `size` bytes.
    pub fn min_fee(&self, size: usize) -> Amount {
        let kib = (size as u128).div_ceil(1024);
        Amount(self.base_fee.0 + self.fee_per_kib.0 * kib)
    }

    /// Digest of the configuration.
    pub fn digest(&self) -> Hash32 {
        Canonical::digest(self, "sion/exec/config")
    }
}

/// The commitment a World Proof attests to.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
    Deserialize,
)]
pub struct WorldCommitment {
    /// The last executed Pulse.
    pub pulse: Pulse,
    /// Root of the object state tree.
    pub state_root: Hash32,
    /// Root of the grow-only knowledge log.
    pub knowledge_root: Hash32,
    /// Number of claims in the knowledge log.
    pub knowledge_size: u64,
    /// Digest of the chain configuration.
    pub config: Hash32,
}

impl WorldCommitment {
    /// Digest of the commitment.
    pub fn digest(&self) -> Hash32 {
        hash_domain("sion/exec/world", &encode(self))
    }
}

/// The full state held by a full node.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct State {
    /// Configuration.
    pub config: ChainConfig,
    /// The last executed Pulse.
    pub pulse: Pulse,
    /// All objects, tombstones included. Write through [`State::put_object`]
    /// so the state tree stays in step.
    pub objects: BTreeMap<ObjectId, Object>,
    /// All claims, by identifier.
    pub claims: BTreeMap<ClaimId, StoredClaim>,
    /// The knowledge log.
    pub knowledge: AppendLog,
    /// The state tree: object ID to object digest.
    pub tree: SparseMerkleTree,
}

impl ObjectStore for State {
    fn get_object(&self, id: &ObjectId) -> Result<Option<Object>, ExecError> {
        Ok(self.objects.get(id).cloned())
    }
}

impl State {
    /// An empty state (use [`crate::genesis`] to build a usable one).
    pub fn empty(config: ChainConfig) -> Self {
        State {
            config,
            pulse: Pulse(0),
            objects: BTreeMap::new(),
            claims: BTreeMap::new(),
            knowledge: AppendLog::new(),
            tree: SparseMerkleTree::new(),
        }
    }

    /// Writes an object and updates the state tree.
    pub fn put_object(&mut self, object: Object) {
        self.tree.insert(object.id.0, object.digest());
        self.objects.insert(object.id, object);
    }

    /// Recomputes the state tree from the objects (after loading a state
    /// from an untrusted source, for example).
    pub fn rebuild_tree(&mut self) {
        let mut tree = SparseMerkleTree::new();
        for (id, object) in &self.objects {
            tree.insert(id.0, object.digest());
        }
        self.tree = tree;
    }

    /// Whether the state tree matches the objects.
    pub fn tree_is_consistent(&self) -> bool {
        self.tree.len() == self.objects.len()
            && self
                .objects
                .iter()
                .all(|(id, object)| self.tree.get(&id.0) == Some(&object.digest()))
    }

    /// The state root.
    pub fn state_root(&self) -> Hash32 {
        self.tree.root()
    }

    /// The world commitment.
    pub fn commitment(&self) -> WorldCommitment {
        WorldCommitment {
            pulse: self.pulse,
            state_root: self.state_root(),
            knowledge_root: self.knowledge.root(),
            knowledge_size: self.knowledge.len(),
            config: self.config.digest(),
        }
    }

    /// The assurance domain.
    pub fn assurance(&self) -> &AssuranceDomain {
        match self.objects.get(&well_known::registry()).map(|o| &o.data) {
            Some(ObjectData::Assurance(domain)) => domain,
            _ => panic!("genesis always creates the assurance domain"),
        }
    }

    /// The assurance registry.
    pub fn registry(&self) -> &Registry {
        &self.assurance().registry
    }

    /// The treasury.
    pub fn treasury(&self) -> &Treasury {
        match self.objects.get(&well_known::treasury()).map(|o| &o.data) {
            Some(ObjectData::Treasury(treasury)) => treasury,
            _ => panic!("genesis always creates the treasury"),
        }
    }

    /// Live coins owned by `account`.
    pub fn coins_of(&self, account: &AccountId) -> Vec<&Object> {
        self.objects
            .values()
            .filter(|o| o.owner == crate::object::Owner::Account(*account) && o.as_coin().is_some())
            .collect()
    }

    /// `account`'s balance of `asset`.
    pub fn balance(&self, account: &AccountId, asset: &AssetId) -> Amount {
        Amount(
            self.coins_of(account)
                .iter()
                .filter_map(|o| o.as_coin())
                .filter(|c| c.asset == *asset)
                .map(|c| c.amount.0)
                .sum(),
        )
    }

    /// Everything of `asset` held anywhere: coins, allotments, bounded
    /// totals' unallocated amounts, market escrow, the assurance vault and
    /// the treasury. Equals the minted supply in a correct state.
    pub fn supply(&self, asset: &AssetId) -> Amount {
        let mut total: u128 = 0;
        for object in self.objects.values() {
            match &object.data {
                ObjectData::Coin(coin) if coin.asset == *asset => total += coin.amount.0,
                ObjectData::Allotment(allotment) if allotment.asset == *asset => {
                    total += allotment.remaining.0
                }
                ObjectData::Bounded(bounded) if bounded.asset == *asset => {
                    total += bounded.unallocated.0
                }
                ObjectData::Market(market) => {
                    for batch in market.batches.values() {
                        if matches!(
                            batch.status,
                            crate::market::BatchStatus::Cleared { .. }
                                | crate::market::BatchStatus::Refunded
                        ) {
                            continue;
                        }
                        for order in &batch.orders {
                            if order.escrow_asset == *asset {
                                total += order.escrow.0;
                            }
                        }
                    }
                }
                ObjectData::Assurance(domain) if *asset == AssetId::usd() => {
                    total += domain.vault.0
                }
                ObjectData::Treasury(treasury) => {
                    total += treasury.balances.get(asset).map(|a| a.0).unwrap_or(0);
                }
                ObjectData::Validators(set) => {
                    for record in set.validators.values() {
                        if set.params.stake_asset == *asset {
                            total += record.stake.0;
                        }
                        if *asset == AssetId::usd() {
                            total += record.rewards.0;
                        }
                    }
                }
                _ => {}
            }
        }
        Amount(total)
    }

    /// The validator set.
    pub fn validators(&self) -> &crate::validators::ValidatorSet {
        match self.objects.get(&well_known::validators()).map(|o| &o.data) {
            Some(ObjectData::Validators(set)) => set,
            _ => panic!("genesis always creates the validator set"),
        }
    }

    /// The forced-inclusion obligations.
    pub fn forced(&self) -> &crate::validators::ForcedObligations {
        match self.objects.get(&well_known::forced()).map(|o| &o.data) {
            Some(ObjectData::Forced(obligations)) => obligations,
            _ => panic!("genesis always creates the forced-inclusion obligations"),
        }
    }

    /// The minted supply of `asset`.
    pub fn minted(&self, asset: &AssetId) -> Amount {
        self.treasury()
            .minted
            .get(asset)
            .copied()
            .unwrap_or(Amount::ZERO)
    }

    /// A claim by identifier.
    pub fn claim(&self, id: &ClaimId) -> Option<&StoredClaim> {
        self.claims.get(id)
    }
}
