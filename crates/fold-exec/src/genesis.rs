//! Genesis: the state every node starts from.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use fold_assurance::atom::standard;
use fold_assurance::{Atom, Registry};
use fold_core::encode::encode;
use fold_core::hash::{hash_domain, Hash32};
use fold_core::ids::{AccountId, AssetId, ObjectId, Pulse, ValidatorId, Version};
use fold_core::Amount;
use fold_crypto::{AuthPolicy, PublicKey};
use serde::{Deserialize, Serialize};

use crate::account::AccountState;
use crate::assurance::AssuranceDomain;
use crate::error::ExecError;
use crate::object::{
    well_known, Coin, Object, ObjectData, Owner, Schedule, ScheduledEvent, Treasury,
};
use crate::state::{ChainConfig, State};
use crate::validators::{
    weight, ForcedObligations, ForcedParams, ValidatorParams, ValidatorRecord, ValidatorSet,
    ValidatorStatus,
};

/// An account that exists at genesis.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct GenesisAccount {
    /// Its authorization policy (the account ID is its hash).
    pub policy: AuthPolicy,
    /// Opening coins.
    pub coins: Vec<(AssetId, Amount)>,
}

impl GenesisAccount {
    /// The account's identifier.
    pub fn id(&self) -> AccountId {
        self.policy.account_id()
    }
}

/// A validator in the genesis active set. Its stake is minted at genesis.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct GenesisValidator {
    /// Its identifier: the hash of its keys, or for a devnet that signs with
    /// the insecure simulation scheme, its simulation identity.
    pub id: ValidatorId,
    /// ML-DSA-44 public key.
    pub lattice: PublicKey,
    /// SLH-DSA public key.
    pub hash: PublicKey,
    /// Its operator.
    pub operator: AccountId,
    /// Stake in USD.
    pub stake: Amount,
    /// Tip of its beacon hash chain.
    pub beacon_tip: Hash32,
}

/// Everything genesis depends on.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct GenesisSpec {
    /// Chain configuration.
    pub config: ChainConfig,
    /// Opening accounts.
    pub accounts: Vec<GenesisAccount>,
    /// Atoms registered at genesis beyond the standard set (governance-set
    /// attack costs live here).
    pub atoms: Vec<Atom>,
    /// The validators of the first epoch.
    pub validators: Vec<GenesisValidator>,
    /// Stake, epoch and committee parameters.
    pub validator_params: ValidatorParams,
    /// Forced-inclusion parameters.
    pub forced: ForcedParams,
}

impl GenesisSpec {
    /// Digest of the specification (the chain's identity).
    pub fn digest(&self) -> Hash32 {
        hash_domain("fold/exec/genesis", &encode(self))
    }
}

/// The atoms every deployment registers: hash functions, lattices, the
/// specification, both proof stacks and the common TEE vendors.
pub fn standard_atoms() -> Vec<Atom> {
    vec![
        standard::sha3(),
        standard::sha2(),
        standard::module_lattice(),
        standard::specification(),
        standard::proof_stack("A"),
        standard::proof_stack("B"),
        standard::tee_resists_host("nvidia-h100"),
        standard::tee_resists_host("intel-tdx"),
        standard::tee_resists_host("amd-sev-snp"),
    ]
}

/// The identifier of an opening coin.
pub fn genesis_coin_id(account: &AccountId, index: u32) -> ObjectId {
    ObjectId::derive_parts(&[b"genesis-coin", &account.0 .0, &index.to_be_bytes()])
}

/// Builds the genesis state.
pub fn genesis(spec: &GenesisSpec) -> Result<State, ExecError> {
    let mut state = State::empty(spec.config.clone());
    let mut registry = Registry::new();
    for atom in standard_atoms()
        .into_iter()
        .chain(spec.atoms.iter().cloned())
    {
        registry.register_atom(atom)?;
    }
    state.put_object(Object {
        id: well_known::registry(),
        version: Version(0),
        owner: Owner::Shared(well_known::assurance_domain()),
        data: ObjectData::Assurance(Box::new(AssuranceDomain {
            registry,
            vault: Amount::ZERO,
            deposits: BTreeMap::new(),
        })),
    });
    let mut minted: BTreeMap<AssetId, Amount> = BTreeMap::new();
    let mut seen = BTreeMap::new();
    for account in &spec.accounts {
        account.policy.validate()?;
        let id = account.id();
        if seen.insert(id, ()).is_some() {
            return Err(ExecError::Invalid(format!(
                "genesis account {id} listed twice"
            )));
        }
        state.put_object(Object {
            id: well_known::account(&id),
            version: Version(0),
            owner: Owner::Account(id),
            data: ObjectData::Account(AccountState {
                id,
                policy: account.policy.clone(),
                viewing_key: None,
                created_at: Pulse(0),
                rotations: 0,
            }),
        });
        for (index, (asset, amount)) in account.coins.iter().enumerate() {
            if *amount == Amount::ZERO {
                continue;
            }
            let entry = minted.entry(*asset).or_insert(Amount::ZERO);
            *entry = entry.checked_add(*amount)?;
            state.put_object(Object {
                id: genesis_coin_id(&id, index as u32),
                version: Version(0),
                owner: Owner::Account(id),
                data: ObjectData::Coin(Coin {
                    asset: *asset,
                    amount: *amount,
                }),
            });
        }
    }
    if let Some(faucet) = spec.config.faucet {
        if !seen.contains_key(&faucet) {
            return Err(ExecError::Invalid(
                "the faucet must be a genesis account".into(),
            ));
        }
    }
    // The first epoch's validators, their stake minted here; the first
    // epoch's committees are sampled with the genesis digest.
    spec.validator_params.check().map_err(ExecError::Invalid)?;
    if spec.validators.is_empty() {
        return Err(ExecError::Invalid("genesis needs a validator".into()));
    }
    if spec.forced.window == 0 {
        return Err(ExecError::Invalid(
            "the forced-inclusion window must be positive".into(),
        ));
    }
    let mut validators = BTreeMap::new();
    for validator in &spec.validators {
        if validator.stake == Amount::ZERO {
            return Err(ExecError::Invalid("a genesis validator needs stake".into()));
        }
        let entry = minted
            .entry(spec.validator_params.stake_asset)
            .or_insert(Amount::ZERO);
        *entry = entry.checked_add(validator.stake)?;
        let record = ValidatorRecord {
            id: validator.id,
            lattice: validator.lattice.clone(),
            hash: validator.hash.clone(),
            operator: validator.operator,
            stake: validator.stake,
            status: ValidatorStatus::Active,
            beacon_tip: validator.beacon_tip,
            rewards: Amount::ZERO,
            registered_at: Pulse(0),
        };
        if validators.insert(validator.id, record).is_some() {
            return Err(ExecError::Invalid(format!(
                "genesis validator {} listed twice",
                validator.id.short()
            )));
        }
    }
    let mut active: Vec<(ValidatorId, u64)> = validators
        .values()
        .map(|r| (r.id, weight(r.stake)))
        .collect();
    active.sort();
    let set = ValidatorSet {
        params: spec.validator_params.clone(),
        epoch: 0,
        epoch_start: Pulse(1),
        seed: spec.digest(),
        active,
        validators,
        rotations: BTreeMap::new(),
        treasury_mark: Amount::ZERO,
    };
    let mut schedule = Schedule::default();
    schedule
        .due
        .entry(set.epoch_end().0)
        .or_default()
        .insert(ScheduledEvent::EpochEnd);
    state.put_object(Object {
        id: well_known::schedule(),
        version: Version(0),
        owner: Owner::Shared(well_known::schedule_domain()),
        data: ObjectData::Schedule(schedule),
    });
    state.put_object(Object {
        id: well_known::validators(),
        version: Version(0),
        owner: Owner::Shared(well_known::validators_domain()),
        data: ObjectData::Validators(Box::new(set)),
    });
    state.put_object(Object {
        id: well_known::forced(),
        version: Version(0),
        owner: Owner::Shared(well_known::forced_domain()),
        data: ObjectData::Forced(Box::new(ForcedObligations {
            params: spec.forced.clone(),
            pending: BTreeMap::new(),
        })),
    });
    state.put_object(Object {
        id: well_known::treasury(),
        version: Version(0),
        owner: Owner::Shared(well_known::treasury_domain()),
        data: ObjectData::Treasury(Treasury {
            balances: BTreeMap::new(),
            minted,
        }),
    });
    Ok(state)
}

/// Deterministic genesis validators for simulations and devnets: `count`
/// validators with keys derived from `name`, each staking `stake` USD and
/// operated by `operator`. Returns them with their keypairs.
pub fn named_validators(
    name: &str,
    count: usize,
    stake: Amount,
    operator: AccountId,
) -> Vec<(GenesisValidator, fold_crypto::DualKeypair)> {
    (0..count)
        .map(|index| {
            let seed = hash_domain(
                "fold/genesis/validator-seed",
                format!("{name}/{index}").as_bytes(),
            );
            let keys =
                fold_crypto::DualKeypair::from_seed(&seed.0, fold_crypto::Scheme::SlhDsaSha2_128f);
            let (lattice, hash) = (keys.lattice.public().clone(), keys.hash.public().clone());
            let validator = GenesisValidator {
                id: fold_crypto::validator_id(&lattice, &hash),
                lattice,
                hash,
                operator,
                stake,
                beacon_tip: hash_domain("fold/genesis/beacon-tip", &seed.0),
            };
            (validator, keys)
        })
        .collect()
}
