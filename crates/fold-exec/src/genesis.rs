//! Genesis: the state every node starts from.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use fold_assurance::atom::standard;
use fold_assurance::{Atom, Registry};
use fold_core::encode::encode;
use fold_core::hash::{hash_domain, Hash32};
use fold_core::ids::{AccountId, AssetId, ObjectId, Pulse, Version};
use fold_core::Amount;
use fold_crypto::AuthPolicy;
use serde::{Deserialize, Serialize};

use crate::account::AccountState;
use crate::assurance::AssuranceDomain;
use crate::error::ExecError;
use crate::object::{well_known, Coin, Object, ObjectData, Owner, Schedule, Treasury};
use crate::state::{ChainConfig, State};

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
    state.put_object(Object {
        id: well_known::schedule(),
        version: Version(0),
        owner: Owner::Shared(well_known::schedule_domain()),
        data: ObjectData::Schedule(Schedule::default()),
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
