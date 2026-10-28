//! Building and signing transactions: coin selection, fees and inputs.
//!
//! A wallet holds an account's dual keypair and knows how to turn an
//! operation into a valid transaction against a view of the state: it picks
//! the gas coin, adds the coins an operation draws from, adds the extra
//! owned inputs an operation needs (an allotment, the account object),
//! computes the minimum fee and signs. Coins it has put into transactions
//! not yet executed are held back, so two transactions built in the same
//! Pulse never spend the same coin version.

use std::collections::BTreeSet;

use sion_clearing::Side;
use sion_core::ids::{AccountId, AssetId, ObjectId, Pulse};
use sion_core::Amount;
use sion_crypto::{AuthPolicy, DualKeypair, Scheme};

use crate::error::ExecError;
use crate::market::Submission;
use crate::object::{well_known, Object, ObjectData, ObjectRef};
use crate::state::{ChainConfig, State};
use crate::tx::{SignedTransaction, Transaction, TxKind};

/// What a wallet needs to know about the state.
pub trait StateView {
    /// The chain configuration.
    fn config(&self) -> &ChainConfig;
    /// The last executed Pulse.
    fn current_pulse(&self) -> Pulse;
    /// An object.
    fn object(&self, id: &ObjectId) -> Option<Object>;
    /// Live coins owned by `account`.
    fn coins(&self, account: &AccountId) -> Vec<Object>;
}

impl StateView for State {
    fn config(&self) -> &ChainConfig {
        &self.config
    }

    fn current_pulse(&self) -> Pulse {
        self.pulse
    }

    fn object(&self, id: &ObjectId) -> Option<Object> {
        self.objects.get(id).cloned()
    }

    fn coins(&self, account: &AccountId) -> Vec<Object> {
        self.coins_of(account).into_iter().cloned().collect()
    }
}

/// An account's keys and transaction builder.
#[derive(Clone, Debug)]
pub struct Wallet {
    /// The keys.
    pub keys: DualKeypair,
    /// The genesis policy (its hash is the account ID).
    pub policy: AuthPolicy,
    /// The account.
    pub id: AccountId,
    /// Coin versions committed to transactions not yet executed.
    in_flight: BTreeSet<ObjectRef>,
    /// Distinguishes otherwise identical transactions.
    nonce: u64,
    /// How many Pulses a transaction stays valid.
    pub validity: u64,
}

impl Wallet {
    /// A wallet from a 32-byte seed. Uses SLH-DSA-SHA2-128f for the hash leg
    /// (fast signing, for busy accounts and simulations).
    pub fn from_seed(seed: &[u8; 32]) -> Wallet {
        Self::with_keys(DualKeypair::from_seed(seed, Scheme::SlhDsaSha2_128f))
    }

    /// A wallet from a name (deterministic; for devnets, tests and demos only).
    pub fn named(name: &str) -> Wallet {
        let digest = sion_core::hash::hash_domain("sion/wallet/devnet-seed", name.as_bytes());
        Self::from_seed(&digest.0)
    }

    /// A wallet around existing keys.
    pub fn with_keys(keys: DualKeypair) -> Wallet {
        let policy = keys.policy();
        let id = policy.account_id();
        Wallet {
            keys,
            policy,
            id,
            in_flight: BTreeSet::new(),
            nonce: 0,
            validity: 20,
        }
    }

    /// Forgets coins held back for pending transactions (after they executed).
    pub fn settle(&mut self) {
        self.in_flight.clear();
    }

    /// Spendable coins of `asset`, largest first, excluding those in flight.
    pub fn spendable(&self, view: &impl StateView, asset: &AssetId) -> Vec<(ObjectRef, Amount)> {
        let mut coins: Vec<(ObjectRef, Amount)> = view
            .coins(&self.id)
            .into_iter()
            .filter_map(|o| {
                let coin = o.as_coin()?;
                (coin.asset == *asset && !self.in_flight.contains(&o.reference()))
                    .then(|| (o.reference(), coin.amount))
            })
            .collect();
        coins.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        coins
    }

    /// Balance of `asset` (including coins in flight).
    pub fn balance(&self, view: &impl StateView, asset: &AssetId) -> Amount {
        Amount(
            view.coins(&self.id)
                .iter()
                .filter_map(|o| o.as_coin())
                .filter(|c| c.asset == *asset)
                .map(|c| c.amount.0)
                .sum(),
        )
    }

    /// What an operation draws from the sender's coins, beyond the fee.
    fn draws(view: &impl StateView, kind: &TxKind) -> Vec<(AssetId, Amount)> {
        match kind {
            TxKind::Transfer { asset, outputs } => {
                vec![(*asset, Amount(outputs.iter().map(|(_, a)| a.0).sum()))]
            }
            TxKind::PostBond { amount, .. } => vec![(AssetId::usd(), *amount)],
            TxKind::Challenge { deposit, .. } => vec![(AssetId::usd(), *deposit)],
            TxKind::CreateCharter {
                funding,
                agent_bond,
                ..
            }
            | TxKind::RegisterAgent {
                funding,
                agent_bond,
                ..
            } => {
                vec![(AssetId::usd(), Amount(funding.0 + agent_bond.0))]
            }
            TxKind::CreateBounded { asset, total, .. } => vec![(*asset, *total)],
            TxKind::OpenStream { asset, deposit, .. } => vec![(*asset, *deposit)],
            TxKind::PostTask {
                amount,
                charter: None,
                ..
            } => vec![(AssetId::usd(), *amount)],
            TxKind::TopUpStream {
                stream, deposit, ..
            } if *deposit > Amount::ZERO => match view.object(stream).map(|o| o.data) {
                Some(ObjectData::Stream(state)) => vec![(state.asset, *deposit)],
                _ => Vec::new(),
            },
            TxKind::DelegateStake { amount, .. } => {
                let asset = match view.object(&well_known::validators()).map(|o| o.data) {
                    Some(ObjectData::Validators(set)) => set.params.stake_asset,
                    _ => AssetId::usd(),
                };
                vec![(asset, *amount)]
            }
            TxKind::RegisterValidator { stake, .. } => {
                let asset = match view.object(&well_known::validators()).map(|o| o.data) {
                    Some(ObjectData::Validators(set)) => set.params.stake_asset,
                    _ => AssetId::usd(),
                };
                vec![(asset, *stake)]
            }
            TxKind::SubmitOrder { market, submission } => {
                let Some(ObjectData::Market(state)) = view.object(market).map(|o| o.data) else {
                    return Vec::new();
                };
                match submission {
                    Submission::Plain(terms) => vec![terms.escrow(&state)],
                    Submission::Sealed { side, escrow, .. } => {
                        let asset = match side {
                            Side::Buy => state.numeraire,
                            Side::Sell => state.good,
                        };
                        vec![(asset, *escrow)]
                    }
                }
            }
            _ => Vec::new(),
        }
    }

    /// The fee an operation needs beyond the minimum for its size: settling
    /// a stream pays per step checked (spec §5.32).
    fn extra_fee(view: &impl StateView, kind: &TxKind) -> Amount {
        match kind {
            TxKind::SettleStream {
                stream,
                position,
                jump,
                ..
            } => match view.object(stream).map(|o| o.data) {
                Some(ObjectData::Stream(state)) => {
                    let hashes = match jump {
                        Some(jump) => {
                            crate::stream::jump_hashes(&state, *position, jump).unwrap_or(0)
                        }
                        None => position.saturating_sub(state.position),
                    };
                    Amount(
                        crate::stream::STREAM_STEP_FEE
                            .0
                            .saturating_mul(u128::from(hashes)),
                    )
                }
                _ => Amount::ZERO,
            },
            _ => Amount::ZERO,
        }
    }

    /// Builds and signs a transaction for `kind`. `extra` are owned inputs
    /// the operation needs after the coins (an allotment for agent payments,
    /// allotment spends, delegation and returns).
    pub fn build(
        &mut self,
        view: &impl StateView,
        kind: TxKind,
        extra: Vec<ObjectRef>,
    ) -> Result<SignedTransaction, ExecError> {
        let config = view.config().clone();
        let account_object = view.object(&well_known::account(&self.id));
        let genesis_policy = account_object.is_none().then(|| self.policy.clone());
        let draws = Self::draws(view, &kind);
        let usd_draw = draws
            .iter()
            .filter(|(asset, _)| *asset == AssetId::usd())
            .map(|(_, a)| a.0)
            .sum::<u128>();
        let usd = self.spendable(view, &AssetId::usd());
        let other: Vec<(AssetId, Amount)> = draws
            .iter()
            .filter(|(a, _)| *a != AssetId::usd())
            .copied()
            .collect();
        let mut extra = extra;
        if matches!(kind, TxKind::RotatePolicy { .. }) {
            if let Some(account) = &account_object {
                extra.push(account.reference());
            }
        }

        let nonce = self.nonce;
        let expires_at = Pulse(view.current_pulse().0 + self.validity);
        let extra_fee = Self::extra_fee(view, &kind);
        let mut fee = config.min_fee(0).saturating_add(extra_fee);
        for _ in 0..4 {
            // USD coins: enough for the fee plus any USD the operation draws.
            let need = fee.0 + usd_draw;
            let mut inputs = Vec::new();
            let mut covered: u128 = 0;
            for (coin, amount) in &usd {
                if covered >= need && !inputs.is_empty() {
                    break;
                }
                inputs.push(*coin);
                covered += amount.0;
            }
            if covered < need || inputs.is_empty() {
                return Err(ExecError::InsufficientFunds {
                    need: Amount(need).to_string(),
                    have: Amount(covered).to_string(),
                });
            }
            for (asset, amount) in &other {
                let mut got: u128 = 0;
                for (coin, value) in self.spendable(view, asset) {
                    if got >= amount.0 {
                        break;
                    }
                    inputs.push(coin);
                    got += value.0;
                }
                if got < amount.0 {
                    return Err(ExecError::InsufficientFunds {
                        need: amount.to_string(),
                        have: Amount(got).to_string(),
                    });
                }
            }
            // Operations that name an owned object (an allotment, the
            // account object) find it at `inputs[1]`, right after the gas
            // coin, however many coins follow.
            let tail = inputs.split_off(1);
            inputs.extend(extra.iter().copied());
            inputs.extend(tail);
            let tx = Transaction {
                sender: self.id,
                genesis_policy: genesis_policy.clone(),
                inputs,
                fee,
                kind: kind.clone(),
                expires_at,
                nonce,
            };
            let required = config.min_fee(tx.size()).saturating_add(extra_fee);
            if required <= fee {
                let signed = tx.sign(&self.keys, &config)?;
                for input in &signed.tx.inputs {
                    self.in_flight.insert(*input);
                }
                self.nonce += 1;
                return Ok(signed);
            }
            fee = required;
        }
        Err(ExecError::Invalid("fee did not converge".into()))
    }

    /// A faucet transaction (only the configured faucet may send it).
    pub fn faucet(
        &mut self,
        view: &impl StateView,
        grants: Vec<(AccountId, AssetId, Amount)>,
    ) -> Result<SignedTransaction, ExecError> {
        let account = view
            .object(&well_known::account(&self.id))
            .ok_or_else(|| ExecError::Invalid("the faucet account must exist".into()))?;
        let tx = Transaction {
            sender: self.id,
            genesis_policy: None,
            inputs: vec![account.reference()],
            fee: Amount::ZERO,
            kind: TxKind::Faucet { grants },
            expires_at: Pulse(view.current_pulse().0 + self.validity),
            nonce: self.nonce,
        };
        self.nonce += 1;
        Ok(tx.sign(&self.keys, view.config())?)
    }
}
