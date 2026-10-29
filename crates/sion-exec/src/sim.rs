//! A single-process chain for tests, demos and differential testing.
//!
//! **Simulation only.** [`Chain`] executes Pulses directly, without lanes,
//! consensus or data availability: every transaction it is given is treated
//! as certified. It verifies signatures, re-executes every chunk of every
//! Pulse statelessly from its witness and checks the conservation
//! invariants after each Pulse, so it is also the strictest harness for the
//! state machine. The networked devnet lives in `sion-node`.

use std::collections::BTreeMap;

use sion_core::hash::hash_domain;
use sion_core::ids::{AccountId, AssetId, ObjectId, Pulse};
use sion_core::Amount;

use crate::error::ExecError;
use crate::genesis::{genesis, named_validators, GenesisAccount, GenesisSpec};
use crate::object::{ObjectData, ObjectRef};
use crate::pulse::{execute_pulse_traced, ChunkTrace, PulseInput, PulseOutput};
use crate::state::{ChainConfig, State};
use crate::stateless::replay_chunk;
use crate::tx::{SignedTransaction, TxKind};
use crate::validators::{ForcedParams, ValidatorParams};
use crate::wallet::{Keyless, Wallet};

/// Genesis validators in simulations.
pub const SIM_VALIDATORS: usize = 4;

/// Each simulated genesis validator's stake, in USD.
pub const SIM_STAKE: u64 = 100_000;

/// One executed Pulse.
#[derive(Clone, Debug)]
pub struct PulseRecord {
    /// What it produced.
    pub output: PulseOutput,
    /// Its chunk traces.
    pub traces: Vec<ChunkTrace>,
}

/// The name of the faucet account in simulations.
pub const FAUCET: &str = "faucet";

/// A simulated chain with named wallets.
#[derive(Debug)]
pub struct Chain {
    /// The state.
    pub state: State,
    /// The genesis specification.
    pub genesis: GenesisSpec,
    /// Wallets by name (the faucet is [`FAUCET`]).
    pub wallets: BTreeMap<String, Wallet>,
    /// Transactions per chunk.
    pub chunk_size: usize,
    /// Whether to replay every chunk from its witness.
    pub replay: bool,
    /// Every executed Pulse.
    pub history: Vec<PulseRecord>,
    /// The genesis validators' keys, in genesis order.
    pub validator_keys: Vec<sion_crypto::DualKeypair>,
    /// Keyless subagents' transaction builders, by charter.
    pub keyless: BTreeMap<ObjectId, Keyless>,
}

/// Who sends a batch member ([`Chain::batch`]).
#[derive(Clone, Copy, Debug)]
pub enum Sender<'a> {
    /// A named wallet.
    Named(&'a str),
    /// The keyless subagent with this charter.
    Keyless(ObjectId),
}

/// A deterministic beacon for simulations.
pub fn sim_beacon(pulse: Pulse) -> sion_core::hash::Hash32 {
    hash_domain("sion/sim/beacon", &pulse.0.to_be_bytes())
}

impl Chain {
    /// A chain whose genesis gives each named account `usd` USD, split into
    /// `coins` coins (so an account can send several transactions per
    /// Pulse). A faucet account holds ten million USD.
    pub fn new(names: &[&str], usd: u64, coins: u32) -> Chain {
        Self::with_params(
            names,
            usd,
            coins,
            ValidatorParams::devnet(),
            ForcedParams::devnet(),
        )
    }

    /// Like [`Chain::new`], with these validator and forced-inclusion
    /// parameters. The [`SIM_VALIDATORS`] genesis validators are operated by
    /// the faucet account.
    pub fn with_params(
        names: &[&str],
        usd: u64,
        coins: u32,
        validator_params: ValidatorParams,
        forced: ForcedParams,
    ) -> Chain {
        let mut wallets = BTreeMap::new();
        let faucet = Wallet::named(FAUCET);
        let mut accounts = vec![GenesisAccount {
            policy: faucet.policy.clone(),
            coins: vec![(AssetId::usd(), Amount::units(10_000_000))],
        }];
        wallets.insert(FAUCET.to_string(), faucet.clone());
        let coins = coins.max(1);
        for name in names {
            let wallet = Wallet::named(name);
            let per = usd / u64::from(coins);
            let mut split: Vec<(AssetId, Amount)> = (0..coins)
                .map(|_| (AssetId::usd(), Amount::units(per)))
                .collect();
            let rest = usd - per * u64::from(coins);
            if rest > 0 {
                split.push((AssetId::usd(), Amount::units(rest)));
            }
            accounts.push(GenesisAccount {
                policy: wallet.policy.clone(),
                coins: split,
            });
            wallets.insert((*name).to_string(), wallet);
        }
        let (validators, validator_keys): (Vec<_>, Vec<_>) = named_validators(
            "sion-sim",
            SIM_VALIDATORS,
            Amount::units(SIM_STAKE),
            faucet.id,
        )
        .into_iter()
        .unzip();
        let spec = GenesisSpec {
            config: ChainConfig::devnet(Some(faucet.id)),
            accounts,
            atoms: Vec::new(),
            validators,
            validator_params,
            forced,
        };
        let state = genesis(&spec).expect("simulation genesis is valid");
        Chain {
            state,
            genesis: spec,
            wallets,
            chunk_size: 3,
            replay: true,
            history: Vec::new(),
            validator_keys,
            keyless: BTreeMap::new(),
        }
    }

    /// Adds a wallet that is not in genesis (it comes into existence with its
    /// first transaction).
    pub fn add_wallet(&mut self, name: &str) -> AccountId {
        let wallet = Wallet::named(name);
        let id = wallet.id;
        self.wallets.insert(name.to_string(), wallet);
        id
    }

    /// The account of a named wallet.
    pub fn id(&self, name: &str) -> AccountId {
        self.wallets[name].id
    }

    /// A named wallet.
    pub fn wallet(&self, name: &str) -> &Wallet {
        &self.wallets[name]
    }

    /// Balance of a named account.
    pub fn balance(&self, name: &str, asset: &AssetId) -> Amount {
        self.state.balance(&self.id(name), asset)
    }

    /// Builds and signs a transaction from a named wallet.
    pub fn tx(&mut self, name: &str, kind: TxKind) -> Result<SignedTransaction, ExecError> {
        self.tx_with(name, kind, Vec::new())
    }

    /// Builds and signs a transaction with extra owned inputs.
    pub fn tx_with(
        &mut self,
        name: &str,
        kind: TxKind,
        extra: Vec<ObjectRef>,
    ) -> Result<SignedTransaction, ExecError> {
        let wallet = self
            .wallets
            .get_mut(name)
            .ok_or_else(|| ExecError::Invalid(format!("no wallet named {name}")))?;
        wallet.build(&self.state, kind, extra)
    }

    /// Builds `members`, each a sender, a kind and extra owned inputs, and
    /// signs them as one batch with the named wallet `signer` (spec §4.3):
    /// one authorization for all of them. Each pays the fee floor of a
    /// member of a batch of this size. A keyless sender's transactions are
    /// authorized only if `signer` is its nearest keyed ancestor's agent.
    pub fn batch(
        &mut self,
        signer: &str,
        members: Vec<(Sender<'_>, TxKind, Vec<ObjectRef>)>,
    ) -> Result<Vec<SignedTransaction>, ExecError> {
        let size = members.len() as u64;
        let mut bodies = Vec::with_capacity(members.len());
        for (sender, kind, extra) in members {
            let body = match sender {
                Sender::Named(name) => {
                    let wallet = self
                        .wallets
                        .get_mut(name)
                        .ok_or_else(|| ExecError::Invalid(format!("no wallet named {name}")))?;
                    (wallet.draft(&self.state, kind, extra, Some(size))?, None)
                }
                Sender::Keyless(charter) => {
                    let builder = self
                        .keyless
                        .entry(charter)
                        .or_insert_with(|| Keyless::new(charter));
                    (
                        builder.draft(&self.state, kind, extra, size)?,
                        Some(charter),
                    )
                }
            };
            bodies.push(body);
        }
        let wallet = self
            .wallets
            .get(signer)
            .ok_or_else(|| ExecError::Invalid(format!("no wallet named {signer}")))?;
        wallet.sign_batch(&self.state.config, bodies)
    }

    /// The account of the keyless subagent with this charter.
    pub fn keyless_id(&self, charter: &ObjectId) -> AccountId {
        crate::charter::keyless_account(charter)
    }

    /// Executes one Pulse. Transactions that need consensus are ordered as
    /// given; the rest go on the owned path.
    pub fn pulse(&mut self, txs: Vec<SignedTransaction>) -> Result<PulseOutput, ExecError> {
        let (contended, owned): (Vec<_>, Vec<_>) =
            txs.into_iter().partition(|t| t.tx.domain().is_some());
        self.pulse_with(owned, contended)
    }

    /// Executes one Pulse with explicit owned and contended sets.
    pub fn pulse_with(
        &mut self,
        owned: Vec<SignedTransaction>,
        contended: Vec<SignedTransaction>,
    ) -> Result<PulseOutput, ExecError> {
        self.pulse_forced(owned, contended, Vec::new())
    }

    /// Executes one Pulse that also posts `forced` intents.
    pub fn pulse_forced(
        &mut self,
        owned: Vec<SignedTransaction>,
        contended: Vec<SignedTransaction>,
        forced: Vec<SignedTransaction>,
    ) -> Result<PulseOutput, ExecError> {
        let pulse = self.state.pulse.next();
        let input = PulseInput {
            pulse,
            beacon: sim_beacon(pulse),
            owned,
            contended,
            forced,
        };
        let (output, traces) =
            execute_pulse_traced(&mut self.state, &input, true, self.chunk_size)?;
        if self.replay {
            check_traces(&output, &traces)?;
        }
        self.check_invariants().map_err(ExecError::Invalid)?;
        for wallet in self.wallets.values_mut() {
            wallet.settle();
        }
        for builder in self.keyless.values_mut() {
            builder.settle();
        }
        self.history.push(PulseRecord {
            output: output.clone(),
            traces,
        });
        Ok(output)
    }

    /// Runs empty Pulses until the state reaches `pulse`.
    pub fn advance_to(&mut self, pulse: Pulse) -> Result<(), ExecError> {
        while self.state.pulse < pulse {
            self.pulse(Vec::new())?;
        }
        Ok(())
    }

    /// Mints from the faucet in its own Pulse.
    pub fn fund(&mut self, grants: &[(&str, AssetId, Amount)]) -> Result<PulseOutput, ExecError> {
        let grants: Vec<(AccountId, AssetId, Amount)> = grants
            .iter()
            .map(|(name, asset, amount)| (self.id(name), *asset, *amount))
            .collect();
        let faucet = self.wallets.get_mut(FAUCET).expect("faucet wallet");
        let tx = faucet.faucet(&self.state, grants)?;
        self.pulse(vec![tx])
    }

    /// Checks the conservation invariants: every asset's holdings equal its
    /// minted supply, the state tree matches the objects, the assurance
    /// vault holds exactly the bonds, pools and deposits, and no bounded
    /// total is over-allotted.
    pub fn check_invariants(&self) -> Result<(), String> {
        check_invariants(&self.state)
    }
}

/// Checks that a Pulse's traces chain from its start to its end and that
/// every chunk re-executes from its witness alone.
pub fn check_traces(output: &PulseOutput, traces: &[ChunkTrace]) -> Result<(), ExecError> {
    let first = traces
        .first()
        .ok_or_else(|| ExecError::Invalid("no traces".into()))?;
    if first.statement.before != output.before {
        return Err(ExecError::Invalid(
            "first chunk does not start at the Pulse's start".into(),
        ));
    }
    for pair in traces.windows(2) {
        let (a, b) = (&pair[0].statement, &pair[1].statement);
        if a.after != b.before || a.deferred_out != b.deferred_in || a.effects_out != b.effects_in {
            return Err(ExecError::Invalid(format!(
                "chunks {} and {} do not chain",
                a.index, b.index
            )));
        }
    }
    let last = traces.last().expect("non-empty");
    if !last.statement.deferred_phase || last.statement.after != output.after {
        return Err(ExecError::Invalid(
            "last chunk does not end at the Pulse's end".into(),
        ));
    }
    for trace in traces {
        replay_chunk(trace)?;
    }
    Ok(())
}

/// The conservation invariants (see [`Chain::check_invariants`]).
pub fn check_invariants(state: &State) -> Result<(), String> {
    let treasury = state.treasury();
    for asset in treasury.minted.keys() {
        let supply = state.supply(asset);
        let minted = state.minted(asset);
        if supply != minted {
            return Err(format!(
                "asset {asset}: holdings {supply} differ from minted {minted}"
            ));
        }
    }
    if !state.tree_is_consistent() {
        return Err("state tree does not match the objects".into());
    }
    let domain = state.assurance();
    let mut held: u128 = 0;
    for bond in domain.registry.bonds() {
        held += bond.amount.saturating_sub(bond.slashed).0 + bond.pool.0;
    }
    held += domain.deposits.values().map(|(_, a)| a.0).sum::<u128>();
    if held != domain.vault.0 {
        return Err(format!(
            "assurance vault holds {} but bonds, pools and deposits total {}",
            domain.vault,
            Amount(held)
        ));
    }
    for object in state.objects.values() {
        if let ObjectData::Bounded(bounded) = &object.data {
            if !bounded.invariant_holds() {
                return Err(format!("bounded total {} over-allotted", object.id));
            }
        }
    }
    Ok(())
}
