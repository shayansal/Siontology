//! A ledger the CLI can talk to: a node over HTTP, or an in-process devnet.

use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use sion_assurance::Policy;
use sion_core::hash::Hash32;
use sion_core::ids::{AccountId, AssetId, ClaimId, ObjectId, Pulse, TxDigest};
use sion_core::Amount;
use sion_da::{DaHeader, Share};
use sion_exec::{ChainConfig, Object, SignedTransaction};
use sion_node::view::{AccountView, EventView, PulseView, SubmitReceipt, TxView};
use sion_node::{Devnet, Query, TxStatus};

use crate::error::CliError;

/// Where a transaction is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TxState {
    /// Being certified or sequenced.
    Pending(String),
    /// Certification failed.
    Rejected(String),
    /// Executed.
    Executed(Box<TxView>),
    /// Unknown to the node.
    Unknown,
}

/// What the CLI needs from a ledger. Ledgers are shared across threads (an
/// x402 resource server settles payments while its client keeps working).
pub trait Ledger: Sync {
    /// Raw query.
    fn query(&self, query: Query) -> Result<Value, CliError>;
    /// Submits a transaction.
    fn submit(&self, tx: &SignedTransaction) -> Result<SubmitReceipt, CliError>;
    /// Submits every member of one batch (spec §4.3). By default one by
    /// one; a ledger across a network sends the batch's signatures once.
    fn submit_batch(&self, members: &[SignedTransaction]) -> Result<Vec<SubmitReceipt>, CliError> {
        members.iter().map(|member| self.submit(member)).collect()
    }
    /// A transaction's state.
    fn tx(&self, digest: TxDigest) -> Result<TxState, CliError>;
    /// Queues a faucet grant.
    fn faucet(&self, account: AccountId, asset: AssetId, amount: Amount) -> Result<(), CliError>;
    /// Posts a forced intent, bypassing the gateway.
    fn force(&self, tx: &SignedTransaction) -> Result<SubmitReceipt, CliError>;
    /// **Adversary simulation (devnet only):** censor these accounts at the
    /// gateway and/or at the first `validators` validators.
    fn censor(
        &self,
        accounts: &[AccountId],
        gateway: bool,
        validators: usize,
    ) -> Result<(), CliError>;
    /// A human-readable description of the ledger.
    fn describe(&self) -> String;
}

fn parse<T: DeserializeOwned>(value: Value) -> Result<T, CliError> {
    serde_json::from_value(value).map_err(|e| CliError::Protocol(e.to_string()))
}

/// Typed helpers over any ledger.
pub trait LedgerExt: Ledger {
    /// Chain status.
    fn status(&self) -> Result<Value, CliError> {
        self.query(Query::Status)
    }
    /// The chain configuration.
    fn chain_config(&self) -> Result<ChainConfig, CliError> {
        let value = self.query(Query::Genesis)?;
        parse(value["spec"]["config"].clone())
    }
    /// The genesis digest.
    fn genesis_digest(&self) -> Result<Hash32, CliError> {
        let value = self.query(Query::Genesis)?;
        parse(value["digest"].clone())
    }
    /// The last executed Pulse.
    fn pulse(&self) -> Result<Pulse, CliError> {
        parse(self.status()?["pulse"].clone())
    }
    /// An object (with no proof check).
    fn object(&self, id: ObjectId) -> Result<Option<Object>, CliError> {
        let value = self.query(Query::Object(id))?;
        parse(value["proof"]["object"].clone())
    }
    /// An account.
    fn account(&self, id: AccountId) -> Result<AccountView, CliError> {
        parse(self.query(Query::Account(id))?)
    }
    /// A claim's record.
    fn claim(&self, id: ClaimId) -> Result<Value, CliError> {
        self.query(Query::Claim(id))
    }
    /// The decision rule.
    fn decide(&self, claim: ClaimId, value: Amount, policy: Policy) -> Result<Value, CliError> {
        self.query(Query::Decide {
            claim,
            value,
            policy,
        })
    }
    /// A Pulse's summary.
    fn pulse_view(&self, pulse: Option<Pulse>) -> Result<PulseView, CliError> {
        parse(self.query(Query::Pulse(pulse))?)
    }
    /// Data-availability header.
    fn data(&self, pulse: Pulse) -> Result<DaHeader, CliError> {
        parse(self.query(Query::Data(pulse))?)
    }
    /// One data share.
    fn share(&self, pulse: Pulse, row: usize, column: usize) -> Result<Share, CliError> {
        parse(self.query(Query::Share { pulse, row, column })?)
    }
    /// Events.
    fn events(&self, from: Pulse, limit: usize) -> Result<Vec<EventView>, CliError> {
        parse(self.query(Query::Events { from, limit })?)
    }
}

impl<L: Ledger + ?Sized> LedgerExt for L {}

impl Ledger for Devnet {
    fn query(&self, query: Query) -> Result<Value, CliError> {
        Devnet::query(self, query).map_err(CliError::Node)
    }

    fn submit(&self, tx: &SignedTransaction) -> Result<SubmitReceipt, CliError> {
        Devnet::submit(self, tx.clone()).map_err(CliError::Node)
    }

    fn submit_batch(&self, members: &[SignedTransaction]) -> Result<Vec<SubmitReceipt>, CliError> {
        Devnet::submit_batch(self, members.to_vec()).map_err(CliError::Node)
    }

    fn tx(&self, digest: TxDigest) -> Result<TxState, CliError> {
        Ok(match self.status(digest).map_err(CliError::Node)? {
            Some(TxStatus::Executed(view)) => TxState::Executed(view),
            Some(TxStatus::Rejected(reason)) => TxState::Rejected(reason),
            Some(TxStatus::Certifying { approvals, .. }) => {
                TxState::Pending(format!("certifying ({approvals} approvals)"))
            }
            Some(TxStatus::Sequencing) => TxState::Pending("sequencing".into()),
            None => TxState::Unknown,
        })
    }

    fn faucet(&self, account: AccountId, asset: AssetId, amount: Amount) -> Result<(), CliError> {
        Devnet::faucet(self, vec![(account, asset, amount)]).map_err(CliError::Node)
    }

    fn force(&self, tx: &SignedTransaction) -> Result<SubmitReceipt, CliError> {
        Devnet::force(self, tx.clone()).map_err(CliError::Node)
    }

    fn censor(
        &self,
        accounts: &[AccountId],
        gateway: bool,
        validators: usize,
    ) -> Result<(), CliError> {
        Devnet::censor(self, accounts.to_vec(), gateway, validators);
        Ok(())
    }

    fn describe(&self) -> String {
        format!(
            "in-process devnet ({} validator threads)",
            self.shared.validators.len()
        )
    }
}

/// A node reached over HTTP.
#[derive(Clone, Debug)]
pub struct HttpLedger {
    /// Base URL, for example `http://127.0.0.1:8730`.
    pub base: String,
    agent: ureq::Agent,
}

impl HttpLedger {
    /// A client for the node at `base`.
    pub fn new(base: &str) -> HttpLedger {
        HttpLedger {
            base: base.trim_end_matches('/').to_string(),
            agent: ureq::AgentBuilder::new()
                .timeout(std::time::Duration::from_secs(30))
                .build(),
        }
    }

    fn get(&self, path: &str) -> Result<Value, CliError> {
        let url = format!("{}{}", self.base, path);
        match self.agent.get(&url).call() {
            Ok(response) => response
                .into_json()
                .map_err(|e| CliError::Protocol(e.to_string())),
            Err(ureq::Error::Status(_, response)) => {
                let value: Value = response.into_json().unwrap_or(Value::Null);
                Err(CliError::Node(
                    value["error"]
                        .as_str()
                        .unwrap_or("request failed")
                        .to_string(),
                ))
            }
            Err(e) => Err(CliError::Connection(format!("{url}: {e}"))),
        }
    }

    fn post(&self, path: &str, body: Value) -> Result<Value, CliError> {
        let url = format!("{}{}", self.base, path);
        match self.agent.post(&url).send_json(body) {
            Ok(response) => response
                .into_json()
                .map_err(|e| CliError::Protocol(e.to_string())),
            Err(ureq::Error::Status(_, response)) => {
                let value: Value = response.into_json().unwrap_or(Value::Null);
                Err(CliError::Node(
                    value["error"]
                        .as_str()
                        .unwrap_or("request failed")
                        .to_string(),
                ))
            }
            Err(e) => Err(CliError::Connection(format!("{url}: {e}"))),
        }
    }
}

impl Ledger for HttpLedger {
    fn query(&self, query: Query) -> Result<Value, CliError> {
        match query {
            Query::Status => self.get("/v1/status"),
            Query::Account(id) => self.get(&format!("/v1/accounts/{id}")),
            Query::Object(id) => self.get(&format!("/v1/objects/{id}")),
            Query::Claim(id) => self.get(&format!("/v1/claims/{id}")),
            Query::Decide {
                claim,
                value,
                policy,
            } => self.post(
                &format!("/v1/claims/{claim}/decide"),
                json!({ "value": value.0, "policy": policy }),
            ),
            Query::Tx(digest) => {
                let value = self.get(&format!("/v1/transactions/{digest}"))?;
                if value["state"] == "executed" {
                    Ok(value["record"].clone())
                } else {
                    Err(CliError::Node(format!("not executed: {}", value["state"])))
                }
            }
            Query::Pulse(None) => self.get("/v1/pulses/latest"),
            Query::Pulse(Some(n)) => self.get(&format!("/v1/pulses/{n}")),
            Query::Data(n) => self.get(&format!("/v1/pulses/{n}/data")),
            Query::Share { pulse, row, column } => {
                self.get(&format!("/v1/pulses/{pulse}/data/{row}/{column}"))
            }
            Query::Events { from, limit } => {
                self.get(&format!("/v1/events?from={from}&limit={limit}"))
            }
            Query::Validators => self.get("/v1/validators"),
            Query::World(n) => self.get(&format!("/v1/pulses/{n}/world")),
            Query::Genesis => self.get("/v1/genesis"),
            Query::Forced => self.get("/v1/forced"),
            Query::Epoch(epoch) => self.get(&format!("/v1/epochs/{epoch}")),
            Query::Committees => self.get("/v1/committees"),
            Query::Agent(id) => self.get(&format!("/v1/agents/{id}")),
            Query::Deals { open: false } => self.get("/v1/deals"),
            Query::Deals { open: true } => self.get("/v1/deals?open=1"),
        }
    }

    fn submit(&self, tx: &SignedTransaction) -> Result<SubmitReceipt, CliError> {
        let value = self.post(
            "/v1/transactions",
            json!({ "tx": hex::encode(sion_core::encode::encode(tx)) }),
        )?;
        parse(value)
    }

    fn submit_batch(&self, members: &[SignedTransaction]) -> Result<Vec<SubmitReceipt>, CliError> {
        // A whole batch goes in one request, its signatures once; anything
        // else member by member.
        let Some(envelope) = sion_exec::tx::BatchEnvelope::of(members) else {
            return members.iter().map(|member| self.submit(member)).collect();
        };
        let value = self.post(
            "/v1/batches",
            json!({ "batch": hex::encode(sion_core::encode::encode(&envelope)) }),
        )?;
        parse(value["members"].clone())
    }

    fn tx(&self, digest: TxDigest) -> Result<TxState, CliError> {
        let value = match self.get(&format!("/v1/transactions/{digest}")) {
            Ok(value) => value,
            Err(CliError::Node(message)) if message.contains("unknown") => {
                return Ok(TxState::Unknown)
            }
            Err(e) => return Err(e),
        };
        Ok(match value["state"].as_str() {
            Some("executed") => TxState::Executed(Box::new(parse(value["record"].clone())?)),
            Some("rejected") => {
                TxState::Rejected(value["reason"].as_str().unwrap_or("").to_string())
            }
            Some(state) => TxState::Pending(state.to_string()),
            None => TxState::Unknown,
        })
    }

    fn faucet(&self, account: AccountId, asset: AssetId, amount: Amount) -> Result<(), CliError> {
        self.post(
            "/v1/faucet",
            json!({ "account": account, "asset": asset, "amount": amount.0 }),
        )?;
        Ok(())
    }

    fn force(&self, tx: &SignedTransaction) -> Result<SubmitReceipt, CliError> {
        let value = self.post(
            "/v1/forced",
            json!({ "tx": hex::encode(sion_core::encode::encode(tx)) }),
        )?;
        parse(value)
    }

    fn censor(
        &self,
        accounts: &[AccountId],
        gateway: bool,
        validators: usize,
    ) -> Result<(), CliError> {
        self.post(
            "/v1/devnet/censor",
            json!({ "accounts": accounts, "gateway": gateway, "validators": validators }),
        )?;
        Ok(())
    }

    fn describe(&self) -> String {
        format!("node at {}", self.base)
    }
}
