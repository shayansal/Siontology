//! JSON over HTTP.
//!
//! | method | path | what |
//! |---|---|---|
//! | GET | `/v1/status` | chain status |
//! | GET | `/v1/validators` | the chain identity, the validator set, the current committees |
//! | GET | `/v1/epochs/{e}` | the proof linking epoch `e − 1` to `e` (for light clients) |
//! | GET | `/v1/genesis` | the genesis specification and its digest |
//! | GET | `/v1/accounts/{id}` | coins, balances, allotments |
//! | GET | `/v1/objects/{id}` | object with a state proof |
//! | GET | `/v1/claims/{id}` | claim, status, cost of corruption, inclusion proof |
//! | POST | `/v1/claims/{id}/decide` | the decision rule for `{value, policy?}` |
//! | POST | `/v1/transactions` | submit `{tx: hex(borsh(SignedTransaction))}` |
//! | GET | `/v1/transactions/{digest}` | status, effects and receipt proof |
//! | POST | `/v1/faucet` | devnet faucet `{account, amount, asset?}` (micro-units) |
//! | GET | `/v1/pulses/latest`, `/v1/pulses/{n}` | header, certificate, summary |
//! | GET | `/v1/pulses/{n}/data` | data-availability header |
//! | GET | `/v1/pulses/{n}/data/{row}/{column}` | one data share with its proof |
//! | GET | `/v1/pulses/{n}/world` | the World Proof |
//! | GET | `/v1/events?from=&limit=` | events |
//! | POST | `/v1/forced` | post a forced intent `{tx: hex}`, bypassing the gateway |
//! | GET | `/v1/forced` | forced-inclusion obligations: the window, a summary (pending and overdue counts, the overdue root), each pending obligation, and this epoch's shard rotations |
//! | GET | `/v1/committees` | the current committees and registered keys, Borsh in hex (what a gateway in another process follows) |
//! | POST | `/v1/devnet/censor` | **adversary simulation**: `{accounts: [hex], gateway, validators}` |
//!
//! The same routes serve an in-process devnet, one validator of a network
//! ([`crate::node::Node`]) and a gateway running alone
//! ([`crate::node::GatewayNode`]): each is a [`Backend`].

use std::io::Read;
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use serde::Deserialize;
use serde_json::{json, Value};
use sion_assurance::Policy;
use sion_core::encode::decode;
use sion_core::hash::Hash32;
use sion_core::ids::{AccountId, AssetId, ClaimId, ObjectId, Pulse, TxDigest};
use sion_core::Amount;
use sion_exec::SignedTransaction;
use tiny_http::{Header, Method, Request, Response, Server};

use crate::devnet::Devnet;
use crate::error::NodeError;
use crate::view::{Query, SubmitReceipt, TxStatus};

/// What the API serves from.
pub trait Backend: Send + Sync + 'static {
    /// Answers a query about the state.
    fn query(&self, query: Query) -> Result<Value, String>;
    /// Submits a signed transaction through a gateway.
    fn submit(&self, tx: SignedTransaction) -> Result<SubmitReceipt, String>;
    /// Where a transaction is.
    fn status(&self, digest: TxDigest) -> Result<Option<TxStatus>, String>;
    /// Queues faucet grants (test networks).
    fn faucet(&self, grants: Vec<(AccountId, AssetId, Amount)>) -> Result<(), String>;
    /// Posts a forced intent straight to consensus.
    fn force(&self, tx: SignedTransaction) -> Result<SubmitReceipt, String>;
    /// **Adversary simulation (devnet only).**
    fn censor(
        &self,
        accounts: Vec<AccountId>,
        gateway: bool,
        validators: usize,
    ) -> Result<(), String>;
}

impl Backend for Devnet {
    fn query(&self, query: Query) -> Result<Value, String> {
        Devnet::query(self, query)
    }

    fn submit(&self, tx: SignedTransaction) -> Result<SubmitReceipt, String> {
        Devnet::submit(self, tx)
    }

    fn status(&self, digest: TxDigest) -> Result<Option<TxStatus>, String> {
        Devnet::status(self, digest)
    }

    fn faucet(&self, grants: Vec<(AccountId, AssetId, Amount)>) -> Result<(), String> {
        Devnet::faucet(self, grants)
    }

    fn force(&self, tx: SignedTransaction) -> Result<SubmitReceipt, String> {
        Devnet::force(self, tx)
    }

    fn censor(
        &self,
        accounts: Vec<AccountId>,
        gateway: bool,
        validators: usize,
    ) -> Result<(), String> {
        Devnet::censor(self, accounts, gateway, validators);
        Ok(())
    }
}

/// A running API server.
pub struct ApiHandle {
    server: Arc<Server>,
    threads: Vec<JoinHandle<()>>,
    /// The address it listens on.
    pub address: String,
}

impl std::fmt::Debug for ApiHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ApiHandle({})", self.address)
    }
}

impl ApiHandle {
    /// Stops the server.
    pub fn stop(self) {
        for _ in &self.threads {
            self.server.unblock();
        }
        for thread in self.threads {
            let _ = thread.join();
        }
    }
}

/// Serves the API of `backend` on `address` (for example
/// `127.0.0.1:8730`).
pub fn serve<B: Backend>(backend: Arc<B>, address: &str) -> Result<ApiHandle, NodeError> {
    let server = Arc::new(
        Server::http(address)
            .map_err(|e| NodeError::Config(format!("cannot listen on {address}: {e}")))?,
    );
    let bound = server
        .server_addr()
        .to_ip()
        .map(|a| a.to_string())
        .unwrap_or_else(|| address.to_string());
    let mut threads = Vec::new();
    for worker in 0..4 {
        let server = server.clone();
        let backend = backend.clone();
        threads.push(
            thread::Builder::new()
                .name(format!("api-{worker}"))
                .stack_size(crate::THREAD_STACK_BYTES)
                .spawn(move || {
                    while let Ok(request) = server.recv() {
                        handle(backend.as_ref(), request);
                    }
                })?,
        );
    }
    Ok(ApiHandle {
        server,
        threads,
        address: bound,
    })
}

struct HttpError(u16, String);

impl From<String> for HttpError {
    fn from(message: String) -> Self {
        HttpError(400, message)
    }
}

fn not_found(message: impl Into<String>) -> HttpError {
    HttpError(404, message.into())
}

fn bad(message: impl Into<String>) -> HttpError {
    HttpError(400, message.into())
}

fn hash(text: &str) -> Result<Hash32, HttpError> {
    Hash32::from_hex(text).map_err(|_| bad(format!("{text:?} is not a 64-digit hex identifier")))
}

fn number<T: std::str::FromStr>(text: &str) -> Result<T, HttpError> {
    text.parse()
        .map_err(|_| bad(format!("{text:?} is not a number")))
}

fn query_param<'a>(url: &'a str, name: &str) -> Option<&'a str> {
    let (_, query) = url.split_once('?')?;
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name).then_some(value)
    })
}

/// Adversary simulation: who censors whom.
#[derive(Deserialize)]
struct CensorBody {
    accounts: Vec<String>,
    #[serde(default)]
    gateway: bool,
    #[serde(default)]
    validators: usize,
}

#[derive(Deserialize)]
struct SubmitBody {
    tx: String,
}

#[derive(Deserialize)]
struct FaucetBody {
    account: String,
    amount: u128,
    asset: Option<String>,
}

#[derive(Deserialize)]
struct DecideBody {
    value: u128,
    policy: Option<Policy>,
}

fn body<T: for<'de> Deserialize<'de>>(request: &mut Request) -> Result<T, HttpError> {
    let mut text = String::new();
    request
        .as_reader()
        .take(4 * 1024 * 1024)
        .read_to_string(&mut text)
        .map_err(|e| bad(e.to_string()))?;
    serde_json::from_str(&text).map_err(|e| bad(format!("bad JSON body: {e}")))
}

fn route<B: Backend>(backend: &B, request: &mut Request) -> Result<Value, HttpError> {
    let url = request.url().to_string();
    let path = url.split('?').next().unwrap_or("").trim_end_matches('/');
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    let method = request.method().clone();
    let q = |query: Query| backend.query(query).map_err(not_found);
    match (method, parts.as_slice()) {
        (Method::Get, ["v1", "status"]) => q(Query::Status),
        (Method::Get, ["v1", "validators"]) => q(Query::Validators),
        (Method::Get, ["v1", "epochs", epoch]) => q(Query::Epoch(
            epoch
                .parse::<u64>()
                .map_err(|_| bad("the epoch must be a number"))?,
        )),
        (Method::Get, ["v1", "genesis"]) => q(Query::Genesis),
        (Method::Get, ["v1", "accounts", id]) => q(Query::Account(AccountId::from_hash(hash(id)?))),
        (Method::Get, ["v1", "objects", id]) => q(Query::Object(ObjectId::from_hash(hash(id)?))),
        (Method::Get, ["v1", "claims", id]) => q(Query::Claim(ClaimId::from_hash(hash(id)?))),
        (Method::Post, ["v1", "claims", id, "decide"]) => {
            let claim = ClaimId::from_hash(hash(id)?);
            let decide: DecideBody = body(request)?;
            q(Query::Decide {
                claim,
                value: Amount(decide.value),
                policy: decide.policy.unwrap_or_else(Policy::standard),
            })
        }
        (Method::Post, ["v1", "transactions"]) => {
            let submit: SubmitBody = body(request)?;
            let bytes = hex::decode(submit.tx.trim()).map_err(|_| bad("tx must be hex"))?;
            let tx: SignedTransaction =
                decode(&bytes).map_err(|_| bad("tx is not a Borsh SignedTransaction"))?;
            let receipt = backend.submit(tx)?;
            Ok(serde_json::to_value(receipt).unwrap_or(Value::Null))
        }
        (Method::Post, ["v1", "forced"]) => {
            let submit: SubmitBody = body(request)?;
            let bytes = hex::decode(submit.tx.trim()).map_err(|_| bad("tx must be hex"))?;
            let tx: SignedTransaction =
                decode(&bytes).map_err(|_| bad("tx is not a Borsh SignedTransaction"))?;
            if tx.size() > 256 * 1024 {
                return Err(bad("transaction too large"));
            }
            let receipt = backend.force(tx)?;
            Ok(serde_json::to_value(receipt).unwrap_or(Value::Null))
        }
        (Method::Get, ["v1", "forced"]) => q(Query::Forced),
        (Method::Get, ["v1", "committees"]) => q(Query::Committees),
        (Method::Post, ["v1", "devnet", "censor"]) => {
            let censor: CensorBody = body(request)?;
            let accounts = censor
                .accounts
                .iter()
                .map(|a| hash(a).map(AccountId::from_hash))
                .collect::<Result<Vec<_>, _>>()?;
            backend.censor(accounts, censor.gateway, censor.validators)?;
            Ok(
                json!({ "censoring": censor.accounts.len(), "gateway": censor.gateway, "validators": censor.validators }),
            )
        }
        (Method::Get, ["v1", "transactions", digest]) => {
            let digest = TxDigest::from_hash(hash(digest)?);
            match backend.status(digest)? {
                Some(TxStatus::Executed(view)) => {
                    Ok(json!({ "state": "executed", "record": view }))
                }
                Some(TxStatus::Certifying { shard, approvals }) => {
                    Ok(json!({ "state": "certifying", "shard": shard, "approvals": approvals }))
                }
                Some(TxStatus::Sequencing) => Ok(json!({ "state": "sequencing" })),
                Some(TxStatus::Rejected(reason)) => {
                    Ok(json!({ "state": "rejected", "reason": reason }))
                }
                None => Err(not_found("unknown transaction")),
            }
        }
        (Method::Post, ["v1", "faucet"]) => {
            let faucet: FaucetBody = body(request)?;
            let account = AccountId::from_hash(hash(&faucet.account)?);
            let asset = match faucet.asset {
                Some(text) => AssetId::from_hash(hash(&text)?),
                None => AssetId::usd(),
            };
            if faucet.amount == 0 || faucet.amount > 1_000_000 * 1_000_000 {
                return Err(bad(
                    "amount must be between 1 micro-unit and one million units",
                ));
            }
            backend.faucet(vec![(account, asset, Amount(faucet.amount))])?;
            Ok(json!({ "queued": true }))
        }
        (Method::Get, ["v1", "pulses", "latest"]) => q(Query::Pulse(None)),
        (Method::Get, ["v1", "pulses", n]) => q(Query::Pulse(Some(Pulse(number(n)?)))),
        (Method::Get, ["v1", "pulses", n, "data"]) => q(Query::Data(Pulse(number(n)?))),
        (Method::Get, ["v1", "pulses", n, "data", row, column]) => q(Query::Share {
            pulse: Pulse(number(n)?),
            row: number(row)?,
            column: number(column)?,
        }),
        (Method::Get, ["v1", "pulses", n, "world"]) => q(Query::World(Pulse(number(n)?))),
        (Method::Get, ["v1", "events"]) => {
            let from = query_param(&url, "from")
                .map(number)
                .transpose()?
                .unwrap_or(0u64);
            let limit = query_param(&url, "limit")
                .map(number)
                .transpose()?
                .unwrap_or(200usize);
            q(Query::Events {
                from: Pulse(from),
                limit: limit.min(5_000),
            })
        }
        _ => Err(not_found(format!("no route for {path}"))),
    }
}

fn handle<B: Backend>(backend: &B, mut request: Request) {
    let (status, value) = match route(backend, &mut request) {
        Ok(value) => (200, value),
        Err(HttpError(code, message)) => (code, json!({ "error": message })),
    };
    let body = serde_json::to_string(&value).unwrap_or_else(|_| "{}".into());
    let mut response = Response::from_string(body).with_status_code(status);
    for (name, value) in [
        ("Content-Type", "application/json"),
        ("Access-Control-Allow-Origin", "*"),
    ] {
        if let Ok(header) = Header::from_bytes(name.as_bytes(), value.as_bytes()) {
            response.add_header(header);
        }
    }
    let _ = request.respond(response);
}
