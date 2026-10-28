//! x402: HTTP 402 payments settled on Siontology.
//!
//! [x402](https://www.x402.org) lets an HTTP server ask for payment with
//! status `402 Payment Required` and lets a client, typically an AI agent,
//! pay and retry without accounts, API keys or checkout pages. This adapter
//! speaks its version 2 wire format and adds a Siontology payment scheme:
//!
//! 1. The server answers `402` with a `PAYMENT-REQUIRED` header (base64 JSON
//!    of [`PaymentRequired`]) listing what it accepts: scheme `exact`,
//!    network `sion:<chain>`, asset `USD`, the amount in micro-units and the
//!    payee's account. Every quote carries a fresh nonce.
//! 2. The client signs a Siontology payment without submitting it: a transfer from
//!    its own coins, or a payment under its Agent Charter whose purpose is
//!    `x402:<nonce>`. It retries with a `PAYMENT-SIGNATURE` header (base64
//!    JSON of [`PaymentPayload`]) carrying the signed transaction.
//! 3. The server's [`Facilitator`] checks the transaction against the quote,
//!    submits it, waits for it to execute and verifies its receipt the way a
//!    light client would: the committee certificate of the Pulse, the
//!    certified order of its transactions and the receipt's inclusion proof.
//!    Only then does the server answer `200`, with a `PAYMENT-RESPONSE`
//!    header (base64 JSON of [`SettleResponse`]).
//!
//! **Replay.** Each quote pays for one request: the facilitator forgets a
//! quote when it settles it and accepts each transaction once. A transaction
//! the chain had already seen before it was presented is refused, so an old
//! payment cannot be replayed after a restart either. A charter payment is
//! also bound to its quote by its purpose; a plain transfer carries no memo,
//! so two servers that share a payee account but not a facilitator could
//! each accept the same transfer once.
//!
//! **Limits stay in the ledger.** An agent's charter caps who it may pay and
//! how much. A server cannot overcharge it, and a prompt-injected agent
//! cannot overpay: the ledger refuses the payment and the server never sees
//! the money.
//!
//! **The `stream` scheme: one hash per request.** A client that calls the
//! same server often opens a payment stream to it once (spec §5.31) and
//! then pays each request with the next element of its hash chain: the
//! payload is `{ "stream", "position", "value" }` instead of a transaction.
//! The server's [`StreamDesk`] checks the element with one hash per step
//! against the last one it accepted, serves the request, and settles
//! thousands of payments in one transaction later. The first payment past
//! each of the chain's checkpoints also carries `jump`, the checkpoint's
//! Merkle path, which lets the server settle any number of payments for a
//! bounded number of hashes. No quote nonce is
//! needed, because a chain element pays for one position only; a client that
//! knows the price pays up front, without the `402` round trip. A refused
//! payment's `PAYMENT-RESPONSE` names the position the server had accepted
//! up to, so an honest client resynchronizes (see [`crate::stream`]).

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sion_core::encode::{decode, encode};
use sion_core::hash::{hash_parts, Hash32};
use sion_core::ids::{AccountId, AssetId, ObjectId, TxDigest};
use sion_core::Amount;
use sion_crypto::AuthPolicy;
use sion_exec::object::well_known;
use sion_exec::tx::signing_context;
use sion_exec::{ChainConfig, ExecStatus, ObjectData, SignedTransaction, TxKind};

use crate::error::CliError;
use crate::ledger::{Ledger, LedgerExt, TxState};
use crate::session::{RemoteView, Session};
use crate::stream::{StreamDesk, StreamPayment};
use crate::verify::{verify_receipt_of, VerifiedReceipt};

/// The x402 protocol version this adapter speaks.
pub const X402_VERSION: u32 = 2;
/// Header of a `402` response: base64 JSON of [`PaymentRequired`].
pub const PAYMENT_REQUIRED: &str = "PAYMENT-REQUIRED";
/// Header of a paid request: base64 JSON of [`PaymentPayload`].
pub const PAYMENT_SIGNATURE: &str = "PAYMENT-SIGNATURE";
/// Header of a paid response: base64 JSON of [`SettleResponse`].
pub const PAYMENT_RESPONSE: &str = "PAYMENT-RESPONSE";
/// Pay exactly the quoted amount with one signed transaction.
pub const SCHEME_EXACT: &str = "exact";
/// Pay the quoted amount from a payment stream, one chain element per
/// request.
pub const SCHEME_STREAM: &str = "stream";
/// The asset requirements name (amounts are micro-USD).
pub const ASSET_USD: &str = "USD";

/// Outstanding quotes a facilitator keeps before dropping the oldest.
const MAX_QUOTES: usize = 100_000;

// ----- Wire format -------------------------------------------------------

/// The resource being paid for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceInfo {
    /// Its URL (this adapter uses the path).
    pub url: String,
    /// What it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Its media type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

/// One way a server accepts payment.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaymentRequirements {
    /// Payment scheme (`exact`).
    pub scheme: String,
    /// CAIP-2 network ([`network`]).
    pub network: String,
    /// Asset (`USD`).
    pub asset: String,
    /// Amount in the asset's base units (micro-USD), as a decimal string.
    pub amount: String,
    /// The payee's account, in hex.
    pub pay_to: String,
    /// How long the quote stays valid.
    pub max_timeout_seconds: u64,
    /// Scheme details: `chain` (the full chain identity), `decimals`,
    /// `nonce` and `purpose` (what a charter payment must say).
    #[serde(default)]
    pub extra: Value,
}

/// A `402` response's body and `PAYMENT-REQUIRED` header.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaymentRequired {
    /// Protocol version.
    pub x402_version: u32,
    /// Why an earlier payment was refused, if one was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// What is for sale.
    pub resource: ResourceInfo,
    /// Accepted ways to pay.
    pub accepts: Vec<PaymentRequirements>,
    /// Extensions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Value>,
}

/// A client's payment: the requirements it accepted and the signed
/// transaction (`payload.transaction`, hex of its Borsh encoding).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaymentPayload {
    /// Protocol version.
    pub x402_version: u32,
    /// The resource paid for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<ResourceInfo>,
    /// The requirements accepted, exactly as quoted.
    pub accepted: PaymentRequirements,
    /// Scheme payload: `{ "transaction": "<hex>" }`.
    pub payload: Value,
    /// Extensions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Value>,
}

/// A facilitator's verdict on a payment before settlement.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyResponse {
    /// Whether the payment would be accepted.
    pub is_valid: bool,
    /// Who pays.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payer: Option<String>,
    /// Why not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invalid_reason: Option<String>,
}

/// The settlement result, sent in `PAYMENT-RESPONSE`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettleResponse {
    /// Whether the payment settled.
    pub success: bool,
    /// Why not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_reason: Option<String>,
    /// Who paid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payer: Option<String>,
    /// The transaction digest (empty if nothing was submitted).
    pub transaction: String,
    /// The network.
    pub network: String,
    /// `sion`: the Pulse it executed in and the certificate that covers it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Value>,
}

/// Encodes a header value: base64 of the JSON.
pub fn encode_header<T: Serialize>(value: &T) -> String {
    STANDARD.encode(serde_json::to_vec(value).expect("wire types serialize"))
}

/// Decodes a header value.
pub fn decode_header<T: DeserializeOwned>(text: &str) -> Result<T, CliError> {
    let bytes = STANDARD
        .decode(text.trim())
        .map_err(|e| CliError::Protocol(format!("x402 header is not base64: {e}")))?;
    serde_json::from_slice(&bytes)
        .map_err(|e| CliError::Protocol(format!("x402 header is not valid JSON: {e}")))
}

/// The CAIP-2 identifier of a Siontology chain: `sion:` and the first 16 bytes of
/// its identity in hex (a CAIP-2 reference holds at most 32 characters; the
/// full identity is in `extra.chain`).
pub fn network(chain: &Hash32) -> String {
    format!("sion:{}", &chain.to_hex()[..32])
}

/// What a charter payment for the quote `nonce` must give as its purpose.
pub fn purpose(nonce: &str) -> String {
    format!("x402:{nonce}")
}

fn parse_account(text: &str) -> Result<AccountId, CliError> {
    Hash32::from_hex(text)
        .map(AccountId::from_hash)
        .map_err(|_| CliError::Protocol(format!("{text:?} is not an account")))
}

fn parse_amount(text: &str) -> Result<Amount, CliError> {
    text.parse::<u128>()
        .map(Amount)
        .map_err(|_| CliError::Protocol(format!("{text:?} is not an amount in micro-units")))
}

// ----- Facilitator --------------------------------------------------------

#[derive(Clone, Debug)]
struct Quote {
    requirements: PaymentRequirements,
    resource: String,
    pay_to: AccountId,
    amount: Amount,
    expires: Instant,
}

/// A payment that passed every check that does not need the ledger to run
/// it.
#[derive(Clone, Debug)]
struct Checked {
    signed: SignedTransaction,
    nonce: String,
}

/// Verifies and settles Siontology payments for a resource server.
pub struct Facilitator<'l> {
    ledger: &'l dyn Ledger,
    chain: Hash32,
    config: ChainConfig,
    quotes: Mutex<BTreeMap<String, Quote>>,
    used: Mutex<BTreeSet<TxDigest>>,
    counter: AtomicU64,
    /// How long settlement waits for a certified receipt.
    pub timeout: Duration,
    streams: Option<StreamDesk>,
    /// The latest Pulse seen, and when it was read.
    pulse: Mutex<(Instant, sion_core::ids::Pulse)>,
    tended: Mutex<Instant>,
}

impl std::fmt::Debug for Facilitator<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Facilitator({})", network(&self.chain))
    }
}

impl<'l> Facilitator<'l> {
    /// A facilitator for the chain `ledger` serves. `chain` is the identity
    /// receipts are verified against (pin it; do not take it from the node).
    pub fn new(ledger: &'l dyn Ledger, chain: Hash32) -> Result<Self, CliError> {
        Ok(Facilitator {
            config: ledger.chain_config()?,
            ledger,
            chain,
            quotes: Mutex::new(BTreeMap::new()),
            used: Mutex::new(BTreeSet::new()),
            counter: AtomicU64::new(0),
            timeout: Duration::from_secs(60),
            streams: None,
            pulse: Mutex::new((Instant::now(), ledger.pulse()?)),
            tended: Mutex::new(Instant::now()),
        })
    }

    /// Also accepts payments from streams, settled by `desk`.
    pub fn with_streams(mut self, desk: StreamDesk) -> Self {
        self.streams = Some(desk);
        self
    }

    /// The stream desk, if this facilitator accepts streams.
    pub fn streams(&self) -> Option<&StreamDesk> {
        self.streams.as_ref()
    }

    /// The latest Pulse, read from the ledger at most every 200 ms.
    fn now(&self) -> sion_core::ids::Pulse {
        let mut cached = self.pulse.lock().expect("pulse lock");
        if cached.0.elapsed() > Duration::from_millis(200) {
            if let Ok(pulse) = self.ledger.pulse() {
                *cached = (Instant::now(), pulse);
            }
        }
        cached.1
    }

    /// This facilitator's network identifier.
    pub fn network(&self) -> String {
        network(&self.chain)
    }

    /// Quotes `amount` USD for `resource`, payable to `pay_to` within
    /// `valid_for`.
    pub fn quote(
        &self,
        resource: ResourceInfo,
        pay_to: AccountId,
        amount: Amount,
        valid_for: Duration,
    ) -> PaymentRequired {
        let count = self.counter.fetch_add(1, Ordering::Relaxed);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let nonce = hash_parts(
            "sion/x402/nonce",
            &[
                &self.chain.0,
                resource.url.as_bytes(),
                &count.to_be_bytes(),
                &now.to_be_bytes(),
            ],
        )
        .to_hex()[..32]
            .to_string();
        let requirements = PaymentRequirements {
            scheme: SCHEME_EXACT.into(),
            network: self.network(),
            asset: ASSET_USD.into(),
            amount: amount.0.to_string(),
            pay_to: pay_to.to_string(),
            max_timeout_seconds: valid_for.as_secs().max(1),
            extra: json!({
                "chain": self.chain.to_hex(),
                "decimals": 6,
                "nonce": nonce,
                "purpose": purpose(&nonce),
            }),
        };
        let mut quotes = self.quotes.lock().expect("quotes lock");
        let now = Instant::now();
        quotes.retain(|_, q| q.expires > now);
        while quotes.len() >= MAX_QUOTES {
            let oldest = quotes
                .iter()
                .min_by_key(|(_, q)| q.expires)
                .map(|(n, _)| n.clone())
                .expect("non-empty");
            quotes.remove(&oldest);
        }
        quotes.insert(
            nonce,
            Quote {
                requirements: requirements.clone(),
                resource: resource.url.clone(),
                pay_to,
                amount,
                expires: now + valid_for,
            },
        );
        let mut accepts = vec![requirements];
        if let Some(desk) = &self.streams {
            accepts.push(self.stream_requirements(desk, pay_to, amount, valid_for));
        }
        PaymentRequired {
            x402_version: X402_VERSION,
            error: None,
            resource,
            accepts,
            extensions: None,
        }
    }

    /// What a payment from a stream must name: the same for every request
    /// of a route (no nonce; a chain element pays for one position only).
    fn stream_requirements(
        &self,
        desk: &StreamDesk,
        pay_to: AccountId,
        amount: Amount,
        valid_for: Duration,
    ) -> PaymentRequirements {
        PaymentRequirements {
            scheme: SCHEME_STREAM.into(),
            network: self.network(),
            asset: ASSET_USD.into(),
            amount: amount.0.to_string(),
            pay_to: pay_to.to_string(),
            max_timeout_seconds: valid_for.as_secs().max(1),
            extra: json!({
                "chain": self.chain.to_hex(),
                "decimals": 6,
                "margin": desk.margin,
            }),
        }
    }

    /// Accepts a payment from a stream for one request to `resource` at
    /// `price`, paid to `pay_to`: one hash per step, nothing submitted. The
    /// desk settles later ([`Facilitator::tend`]).
    pub fn accept_stream(
        &self,
        resource: &str,
        price: Amount,
        pay_to: AccountId,
        payload: &PaymentPayload,
    ) -> SettleResponse {
        let failure = |reason: String, extensions: Option<Value>| SettleResponse {
            success: false,
            error_reason: Some(reason),
            payer: None,
            transaction: String::new(),
            network: self.network(),
            extensions,
        };
        let Some(desk) = &self.streams else {
            return failure("this server does not accept streams".into(), None);
        };
        let accepted = &payload.accepted;
        if payload.x402_version != X402_VERSION {
            return failure(
                format!("x402 version {} is not supported", payload.x402_version),
                None,
            );
        }
        if accepted.scheme != SCHEME_STREAM
            || accepted.network != self.network()
            || accepted.asset != ASSET_USD
            || accepted.extra["chain"].as_str() != Some(self.chain.to_hex().as_str())
        {
            return failure(
                format!("not a {SCHEME_STREAM} payment on {}", self.network()),
                None,
            );
        }
        if accepted.amount != price.0.to_string() || accepted.pay_to != pay_to.to_string() {
            return failure(
                format!("{resource} costs {price} USD paid to {}", pay_to.short()),
                None,
            );
        }
        let StreamPayment { payment, jump } = match serde_json::from_value(payload.payload.clone())
        {
            Ok(payment) => payment,
            Err(e) => return failure(format!("the stream payment does not parse: {e}"), None),
        };
        match desk.accept(self.ledger, self.now(), &payment, price, jump.as_ref()) {
            Ok(done) => SettleResponse {
                success: true,
                error_reason: None,
                payer: Some(done.payer.to_string()),
                transaction: String::new(),
                network: self.network(),
                extensions: Some(json!({
                    "sion": {
                        "scheme": SCHEME_STREAM,
                        "stream": payment.stream,
                        "position": done.position,
                        "settled": done.settled,
                    }
                })),
            },
            Err(refused) => failure(
                refused.reason,
                refused.position.map(|position| {
                    json!({ "sion": { "stream": payment.stream, "position": position } })
                }),
            ),
        }
    }

    /// Checks settlements in flight and sends those that are due, at most
    /// every 100 ms. A server calls it between requests.
    pub fn tend(&self) {
        let Some(desk) = &self.streams else {
            return;
        };
        {
            let mut tended = self.tended.lock().expect("tended lock");
            if tended.elapsed() < Duration::from_millis(100) {
                return;
            }
            *tended = Instant::now();
        }
        // A failure here is retried on the next call.
        let _ = desk.refresh(self.ledger);
        let _ = desk.settle(self.ledger, &self.config, self.now(), false);
    }

    /// Settles every unsettled stream payment and waits for the settlements.
    pub fn close_out(&self) -> Result<(), CliError> {
        match &self.streams {
            Some(desk) => desk.close_out(self.ledger, &self.config, self.timeout),
            None => Ok(()),
        }
    }

    /// The sender's authorization policy: its current one, or the genesis
    /// policy the transaction carries if the account is new.
    fn policy_of(&self, signed: &SignedTransaction) -> Result<AuthPolicy, String> {
        let sender = signed.tx.sender;
        let account = self
            .ledger
            .object(well_known::account(&sender))
            .map_err(|e| e.to_string())?;
        match account.map(|o| o.data) {
            Some(ObjectData::Account(account)) => Ok(account.policy),
            Some(_) => Err("the sender's account object is not an account".into()),
            None => {
                let policy = signed
                    .tx
                    .genesis_policy
                    .clone()
                    .ok_or("unknown account and no genesis policy")?;
                if policy.account_id() != sender {
                    return Err("the genesis policy does not hash to the sender".into());
                }
                Ok(policy)
            }
        }
    }

    /// Every check that does not need the payment to run.
    fn check(&self, resource: &str, payload: &PaymentPayload) -> Result<Checked, String> {
        if payload.x402_version != X402_VERSION {
            return Err(format!(
                "x402 version {} is not supported",
                payload.x402_version
            ));
        }
        let accepted = &payload.accepted;
        if accepted.scheme != SCHEME_EXACT {
            return Err(format!("scheme {:?} is not supported", accepted.scheme));
        }
        if accepted.network != self.network() {
            return Err(format!(
                "the payment is for {}, not {}",
                accepted.network,
                self.network()
            ));
        }
        let nonce = accepted.extra["nonce"]
            .as_str()
            .ok_or("the accepted requirements carry no nonce")?
            .to_string();
        let quote = self
            .quotes
            .lock()
            .expect("quotes lock")
            .get(&nonce)
            .cloned()
            .ok_or("unknown quote, or one already used")?;
        if quote.requirements != *accepted {
            return Err("the accepted requirements are not the ones quoted".into());
        }
        if quote.resource != resource {
            return Err(format!(
                "the quote was for {}, not {resource}",
                quote.resource
            ));
        }
        if Instant::now() > quote.expires {
            return Err("the quote expired".into());
        }
        let hex = payload.payload["transaction"]
            .as_str()
            .ok_or("the payload carries no transaction")?;
        let bytes = hex::decode(hex).map_err(|_| "the transaction is not hex")?;
        let signed: SignedTransaction =
            decode(&bytes).map_err(|_| "the transaction does not decode")?;
        match &signed.tx.kind {
            TxKind::Transfer { asset, outputs } => {
                if *asset != AssetId::usd() {
                    return Err("the transfer is not in USD".into());
                }
                let paid: u128 = outputs
                    .iter()
                    .filter(|(to, _)| *to == quote.pay_to)
                    .map(|(_, amount)| amount.0)
                    .sum();
                if paid != quote.amount.0 {
                    return Err(format!(
                        "the transfer pays the payee {} USD, not {} USD",
                        Amount(paid),
                        quote.amount
                    ));
                }
            }
            TxKind::AgentPay {
                payee,
                amount,
                purpose: stated,
                ..
            } => {
                if *payee != quote.pay_to || *amount != quote.amount {
                    return Err(format!(
                        "the charter payment pays {amount} USD to {}, not {} USD to {}",
                        payee.short(),
                        quote.amount,
                        quote.pay_to.short()
                    ));
                }
                if *stated != purpose(&nonce) {
                    return Err("the charter payment's purpose does not name this quote".into());
                }
            }
            _ => return Err("the transaction is not a payment".into()),
        }
        let policy = self.policy_of(&signed)?;
        policy
            .verify(
                &signing_context(&self.config),
                &signed.tx.digest().0 .0,
                &signed.auth,
            )
            .map_err(|_| "the payment's signatures do not verify for this chain")?;
        let pulse = self.ledger.pulse().map_err(|e| e.to_string())?;
        if signed.tx.expires_at <= pulse {
            return Err("the payment has expired".into());
        }
        let digest = signed.digest();
        if self.used.lock().expect("used lock").contains(&digest) {
            return Err("this payment was already used".into());
        }
        match self.ledger.tx(digest).map_err(|e| e.to_string())? {
            TxState::Unknown => {}
            _ => return Err("the ledger saw this transaction before it was presented".into()),
        }
        Ok(Checked { signed, nonce })
    }

    /// Checks a payment for `resource` without settling it.
    pub fn verify(&self, resource: &str, payload: &PaymentPayload) -> VerifyResponse {
        match self.check(resource, payload) {
            Ok(checked) => VerifyResponse {
                is_valid: true,
                payer: Some(checked.signed.tx.sender.to_string()),
                invalid_reason: None,
            },
            Err(reason) => VerifyResponse {
                is_valid: false,
                payer: None,
                invalid_reason: Some(reason),
            },
        }
    }

    /// Checks, submits and waits for a payment for `resource`, then verifies
    /// its receipt against the pinned chain. Each quote and each
    /// transaction settles at most once.
    pub fn settle(&self, resource: &str, payload: &PaymentPayload) -> SettleResponse {
        let failure = |reason: String, transaction: String| SettleResponse {
            success: false,
            error_reason: Some(reason),
            payer: None,
            transaction,
            network: self.network(),
            extensions: None,
        };
        let checked = match self.check(resource, payload) {
            Ok(checked) => checked,
            Err(reason) => return failure(reason, String::new()),
        };
        let digest = checked.signed.digest();
        // Claim the quote and the transaction before anything leaves.
        if self
            .quotes
            .lock()
            .expect("quotes lock")
            .remove(&checked.nonce)
            .is_none()
        {
            return failure("the quote was used concurrently".into(), String::new());
        }
        if !self.used.lock().expect("used lock").insert(digest) {
            return failure("this payment was already used".into(), String::new());
        }
        match self.run(&checked.signed) {
            Ok(receipt) => SettleResponse {
                success: true,
                error_reason: None,
                payer: Some(checked.signed.tx.sender.to_string()),
                transaction: digest.to_string(),
                network: self.network(),
                extensions: Some(json!({
                    "sion": {
                        "pulse": receipt.pulse,
                        "chain": receipt.chain.0,
                        "signedStake": receipt.signed.map(|(s, _)| s),
                        "quorum": receipt.signed.map(|(_, q)| q),
                    }
                })),
            },
            Err(reason) => failure(reason, digest.to_string()),
        }
    }

    /// Submits a checked payment and returns its verified receipt.
    fn run(&self, signed: &SignedTransaction) -> Result<VerifiedReceipt, String> {
        let digest = signed.digest();
        self.ledger.submit(signed).map_err(|e| e.to_string())?;
        let deadline = Instant::now() + self.timeout;
        loop {
            match self.ledger.tx(digest).map_err(|e| e.to_string())? {
                TxState::Executed(view) => match &view.status {
                    ExecStatus::Success => break,
                    ExecStatus::Failed(reason) => {
                        return Err(format!("the ledger refused the payment: {reason}"))
                    }
                    other => return Err(format!("the ledger refused the payment: {other:?}")),
                },
                TxState::Rejected(reason) => {
                    return Err(format!("the payment was not certified: {reason}"))
                }
                TxState::Pending(_) | TxState::Unknown => {}
            }
            if Instant::now() > deadline {
                return Err("the payment did not execute in time".into());
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        // The receipt counts once a committee certificate covers its Pulse.
        loop {
            match verify_receipt_of(self.ledger, Some(self.chain), digest) {
                Ok(receipt) if receipt.effects.status == ExecStatus::Success => return Ok(receipt),
                Ok(_) => return Err("the certified receipt does not show success".into()),
                Err(error) if Instant::now() > deadline => {
                    return Err(format!("no verifiable receipt in time: {error}"))
                }
                Err(_) => std::thread::sleep(Duration::from_millis(25)),
            }
        }
    }
}

// ----- Resource server ------------------------------------------------------

/// A resource sold over x402.
pub struct PaidRoute {
    /// Its path (`/v1/forecast`).
    pub path: String,
    /// Its price in USD.
    pub price: Amount,
    /// What it is.
    pub description: String,
    /// Its media type.
    pub mime_type: String,
    /// Produces the body (given the query string).
    pub body: Box<dyn Fn(&str) -> String + Send + Sync>,
}

impl std::fmt::Debug for PaidRoute {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PaidRoute({} at {} USD)", self.path, self.price)
    }
}

/// An HTTP server that sells resources over x402 and settles on Siontology.
pub struct PaidServer {
    server: tiny_http::Server,
    /// The address it listens on.
    pub address: String,
}

impl std::fmt::Debug for PaidServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PaidServer({})", self.address)
    }
}

fn header(name: &str, value: &str) -> tiny_http::Header {
    tiny_http::Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("valid header")
}

impl PaidServer {
    /// Listens on `address` (`127.0.0.1:0` picks a free port).
    pub fn bind(address: &str) -> Result<PaidServer, CliError> {
        let server = tiny_http::Server::http(address)
            .map_err(|e| CliError::Usage(format!("cannot listen on {address}: {e}")))?;
        let address = server
            .server_addr()
            .to_ip()
            .map(|a| a.to_string())
            .unwrap_or_else(|| address.to_string());
        Ok(PaidServer { server, address })
    }

    /// Serves `routes`, paid to `pay_to`, until `stop` is set.
    pub fn serve(
        &self,
        facilitator: &Facilitator<'_>,
        pay_to: AccountId,
        routes: &[PaidRoute],
        quote_valid_for: Duration,
        stop: &AtomicBool,
    ) {
        while !stop.load(Ordering::Relaxed) {
            let request = match self.server.recv_timeout(Duration::from_millis(50)) {
                Ok(Some(request)) => request,
                Ok(None) => {
                    facilitator.tend();
                    continue;
                }
                Err(_) => break,
            };
            self.answer(request, facilitator, pay_to, routes, quote_valid_for);
            facilitator.tend();
        }
        // Whatever streams paid is settled before the server goes.
        if let Err(error) = facilitator.close_out() {
            eprintln!("warning: {error}");
        }
    }

    fn answer(
        &self,
        mut request: tiny_http::Request,
        facilitator: &Facilitator<'_>,
        pay_to: AccountId,
        routes: &[PaidRoute],
        quote_valid_for: Duration,
    ) {
        let url = request.url().to_string();
        let (path, query) = url.split_once('?').unwrap_or((url.as_str(), ""));
        let Some(route) = routes.iter().find(|r| r.path == path) else {
            let _ = request
                .respond(tiny_http::Response::from_string("not found").with_status_code(404));
            return;
        };
        // Bodies are ignored, but read so the connection stays usable.
        let mut sink = Vec::new();
        let _ = request.as_reader().take(64 * 1024).read_to_end(&mut sink);
        let resource = ResourceInfo {
            url: route.path.clone(),
            description: Some(route.description.clone()),
            mime_type: Some(route.mime_type.clone()),
        };
        let payment = request
            .headers()
            .iter()
            .find(|h| h.field.equiv(PAYMENT_SIGNATURE))
            .map(|h| h.value.as_str().to_string());
        let ask = |error: Option<String>, settlement: Option<&SettleResponse>| {
            let mut required =
                facilitator.quote(resource.clone(), pay_to, route.price, quote_valid_for);
            required.error = error;
            let body = serde_json::to_string(&required).expect("serializable");
            let mut response = tiny_http::Response::from_string(body)
                .with_status_code(402)
                .with_header(header("Content-Type", "application/json"))
                .with_header(header(PAYMENT_REQUIRED, &encode_header(&required)));
            if let Some(settlement) = settlement {
                response =
                    response.with_header(header(PAYMENT_RESPONSE, &encode_header(settlement)));
            }
            response
        };
        let Some(payment) = payment else {
            let _ = request.respond(ask(None, None));
            return;
        };
        let payload: PaymentPayload = match decode_header(&payment) {
            Ok(payload) => payload,
            Err(error) => {
                let _ = request.respond(ask(Some(error.to_string()), None));
                return;
            }
        };
        let settlement = match payload.accepted.scheme.as_str() {
            SCHEME_STREAM => facilitator.accept_stream(&route.path, route.price, pay_to, &payload),
            _ => facilitator.settle(&route.path, &payload),
        };
        if !settlement.success {
            let reason = settlement.error_reason.clone();
            let _ = request.respond(ask(reason, Some(&settlement)));
            return;
        }
        let response = tiny_http::Response::from_string((route.body)(query))
            .with_header(header("Content-Type", &route.mime_type))
            .with_header(header(PAYMENT_RESPONSE, &encode_header(&settlement)));
        let _ = request.respond(response);
    }
}

// ----- Client -------------------------------------------------------------------

/// How a client pays.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Payer {
    /// A transfer from a named wallet's coins.
    Wallet(String),
    /// A payment under an Agent Charter, from the agent's allotment.
    Charter {
        /// The agent's wallet.
        agent: String,
        /// The charter.
        charter: ObjectId,
        /// The agent's allotment.
        allotment: ObjectId,
    },
    /// The next element of a payment stream the session's book holds.
    Stream(ObjectId),
}

impl Payer {
    /// The scheme this payer pays with.
    pub fn scheme(&self) -> &'static str {
        match self {
            Payer::Stream(_) => SCHEME_STREAM,
            _ => SCHEME_EXACT,
        }
    }
}

/// What a paid request returned.
#[derive(Clone, Debug, PartialEq)]
pub struct Fetched {
    /// The final HTTP status.
    pub status: u16,
    /// The final body.
    pub body: String,
    /// The quote the server made, if it asked for payment.
    pub quoted: Option<PaymentRequired>,
    /// The settlement it reported, if the client paid.
    pub settlement: Option<SettleResponse>,
    /// The payment the client signed, if it paid (its `PAYMENT-SIGNATURE`
    /// header value).
    pub payment: Option<String>,
    /// The requirements it paid against (reusable for the next request of a
    /// `stream` payment).
    pub accepted: Option<PaymentRequirements>,
}

fn response_parts(response: ureq::Response) -> (u16, Option<String>, Option<String>, String) {
    let status = response.status();
    let required = response.header(PAYMENT_REQUIRED).map(str::to_string);
    let settled = response.header(PAYMENT_RESPONSE).map(str::to_string);
    let body = response.into_string().unwrap_or_default();
    (status, required, settled, body)
}

fn get(
    agent: &ureq::Agent,
    url: &str,
    payment: Option<&str>,
) -> Result<(u16, Option<String>, Option<String>, String), CliError> {
    let mut request = agent.get(url);
    if let Some(payment) = payment {
        request = request.set(PAYMENT_SIGNATURE, payment);
    }
    match request.call() {
        Ok(response) => Ok(response_parts(response)),
        Err(ureq::Error::Status(_, response)) => Ok(response_parts(response)),
        Err(error) => Err(CliError::Connection(error.to_string())),
    }
}

/// Signs, without submitting, a payment meeting `requirements`.
pub fn sign_payment(
    session: &mut Session<'_>,
    payer: &Payer,
    requirements: &PaymentRequirements,
) -> Result<SignedTransaction, CliError> {
    let pay_to = parse_account(&requirements.pay_to)?;
    let amount = parse_amount(&requirements.amount)?;
    let ledger = session.ledger;
    let view = RemoteView::new(ledger, session.config.clone());
    let signed = match payer {
        Payer::Wallet(name) => session.wallet(name).build(
            &view,
            TxKind::Transfer {
                asset: AssetId::usd(),
                outputs: vec![(pay_to, amount)],
            },
            Vec::new(),
        )?,
        Payer::Stream(_) => {
            return Err(CliError::Usage(
                "a stream payment is not a transaction; see fetch".into(),
            ))
        }
        Payer::Charter {
            agent,
            charter,
            allotment,
        } => {
            let input = session.reference(allotment)?;
            let stated = requirements.extra["purpose"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            session.wallet(agent).build(
                &view,
                TxKind::AgentPay {
                    charter: *charter,
                    payee: pay_to,
                    amount,
                    purpose: stated,
                    evidence: None,
                },
                vec![input],
            )?
        }
    };
    Ok(signed)
}

/// Fetches `url`; if the server asks for payment on `chain` of at most
/// `max_price` USD, pays as `payer` and retries once.
pub fn fetch(
    session: &mut Session<'_>,
    payer: &Payer,
    url: &str,
    chain: &Hash32,
    max_price: Amount,
) -> Result<Fetched, CliError> {
    fetch_with(session, payer, url, chain, max_price, None)
}

fn client() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(120))
        .build()
}

/// Like [`fetch`]; with `known` requirements (from an earlier request's
/// [`Fetched::accepted`]), pays up front without asking first. Only a
/// `stream` payment can be made this way: an `exact` quote pays once.
pub fn fetch_with(
    session: &mut Session<'_>,
    payer: &Payer,
    url: &str,
    chain: &Hash32,
    max_price: Amount,
    known: Option<&PaymentRequirements>,
) -> Result<Fetched, CliError> {
    let agent = client();
    let (requirements, quoted) = match known {
        Some(requirements) if requirements.scheme == SCHEME_STREAM => (requirements.clone(), None),
        Some(_) => {
            return Err(CliError::Usage(
                "only stream requirements can be paid up front".into(),
            ))
        }
        None => {
            let (status, required, _, body) = get(&agent, url, None)?;
            if status != 402 {
                return Ok(Fetched {
                    status,
                    body,
                    quoted: None,
                    settlement: None,
                    payment: None,
                    accepted: None,
                });
            }
            let quoted: PaymentRequired = match required {
                Some(header) => decode_header(&header)?,
                None => serde_json::from_str(&body).map_err(|e| {
                    CliError::Protocol(format!("402 without payment requirements: {e}"))
                })?,
            };
            let ours = network(chain);
            let requirements = quoted
                .accepts
                .iter()
                .find(|r| r.scheme == payer.scheme() && r.network == ours && r.asset == ASSET_USD)
                .ok_or_else(|| {
                    CliError::Protocol(format!(
                        "the server does not accept {} payments on {ours}",
                        payer.scheme()
                    ))
                })?
                .clone();
            (requirements, Some(quoted))
        }
    };
    if requirements.extra["chain"].as_str() != Some(chain.to_hex().as_str()) {
        return Err(CliError::Protocol(
            "the quote names another chain identity".into(),
        ));
    }
    let price = parse_amount(&requirements.amount)?;
    if price > max_price {
        return Err(CliError::Usage(format!(
            "the server asks {price} USD, more than the {max_price} USD this client may pay"
        )));
    }
    let resource = quoted.as_ref().map(|q| q.resource.clone());
    // A stream pays `price / unit` steps per request.
    let steps = match payer {
        Payer::Stream(stream) => {
            let record = session.streams.record(stream)?;
            if record.payee != parse_account(&requirements.pay_to)? {
                return Err(CliError::Usage(format!(
                    "stream {} does not pay {}",
                    stream.short(),
                    requirements.pay_to
                )));
            }
            let unit = record.unit;
            if unit == Amount::ZERO || price.0 % unit.0 != 0 {
                return Err(CliError::Usage(format!(
                    "the price, {price} USD, is not a whole number of the stream's {unit} USD steps"
                )));
            }
            u64::try_from(price.0 / unit.0).map_err(|_| CliError::Usage("too many steps".into()))?
        }
        _ => 0,
    };
    let mut retried = false;
    loop {
        let body = match payer {
            Payer::Stream(stream) => {
                serde_json::to_value(session.streams.pay_with_jump(stream, steps)?)
                    .expect("serializable")
            }
            _ => {
                let signed = sign_payment(session, payer, &requirements)?;
                json!({ "transaction": hex::encode(encode(&signed)) })
            }
        };
        let payload = PaymentPayload {
            x402_version: X402_VERSION,
            resource: resource.clone(),
            accepted: requirements.clone(),
            payload: body,
            extensions: None,
        };
        let payment = encode_header(&payload);
        let result = get(&agent, url, Some(&payment));
        match payer {
            Payer::Wallet(name) | Payer::Charter { agent: name, .. } => {
                session.wallet(name).settle()
            }
            Payer::Stream(_) => {}
        }
        let (status, _, settled, body) = result?;
        let settlement = settled
            .map(|header| decode_header::<SettleResponse>(&header))
            .transpose()?;
        // A refused stream payment names the position the server accepted
        // up to. An honest client goes back there (re-revealing nothing
        // new), and pays once more if it had been ahead of the server.
        if let (Payer::Stream(stream), Some(refused)) = (
            payer,
            settlement.as_ref().filter(|s| !s.success && status == 402),
        ) {
            let theirs = refused
                .extensions
                .as_ref()
                .and_then(|e| e["sion"]["position"].as_u64());
            if let Some(position) = theirs {
                let ours = session.streams.record(stream)?.revealed;
                session.streams.resync(stream, position)?;
                if !retried && position.saturating_add(steps) < ours {
                    retried = true;
                    continue;
                }
            }
        }
        return Ok(Fetched {
            status,
            body,
            quoted,
            settlement,
            payment: Some(payment),
            accepted: Some(requirements),
        });
    }
}

/// Sends a payment header again, as a replaying client would.
pub fn replay(url: &str, payment: &str) -> Result<Fetched, CliError> {
    let (status, required, settled, body) = get(&client(), url, Some(payment))?;
    Ok(Fetched {
        status,
        body,
        quoted: required
            .map(|header| decode_header::<PaymentRequired>(&header))
            .transpose()?,
        settlement: settled
            .map(|header| decode_header::<SettleResponse>(&header))
            .transpose()?,
        payment: Some(payment.to_string()),
        accepted: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headers_round_trip_and_use_the_v2_field_names() {
        let required = PaymentRequired {
            x402_version: X402_VERSION,
            error: None,
            resource: ResourceInfo {
                url: "/v1/forecast".into(),
                description: Some("a forecast".into()),
                mime_type: Some("application/json".into()),
            },
            accepts: vec![PaymentRequirements {
                scheme: SCHEME_EXACT.into(),
                network: network(&Hash32([7; 32])),
                asset: ASSET_USD.into(),
                amount: "250000".into(),
                pay_to: AccountId::derive(b"carol").to_string(),
                max_timeout_seconds: 60,
                extra: json!({ "nonce": "n" }),
            }],
            extensions: None,
        };
        let header = encode_header(&required);
        let back: PaymentRequired = decode_header(&header).unwrap();
        assert_eq!(back, required);
        let json: Value = serde_json::from_slice(&STANDARD.decode(&header).unwrap()).unwrap();
        assert_eq!(json["x402Version"], 2);
        assert_eq!(json["accepts"][0]["payTo"], required.accepts[0].pay_to);
        assert_eq!(json["accepts"][0]["maxTimeoutSeconds"], 60);
        assert!(json.get("error").is_none(), "absent options are omitted");
        assert!(decode_header::<PaymentRequired>("not base64!").is_err());
    }

    #[test]
    fn network_identifiers_are_caip2() {
        let id = network(&Hash32([0xab; 32]));
        let (namespace, reference) = id.split_once(':').unwrap();
        assert_eq!(namespace, "sion");
        assert_eq!(reference.len(), 32);
        assert!(reference.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
