//! The merchant: ACP's checkout sessions, carts and orders, paid on the
//! ledger or by card, its state kept across restarts.
//!
//! **Checkout** (`/checkout_sessions`): create, update, retrieve, complete,
//! cancel. A session is priced on the ledger as an x402 quote
//! ([`super::HANDLER`]); with a card handler ([`CardHandler`]) it can be
//! paid by a delegated card too; with AP2 ([`Merchant::with_ap2`]) its
//! completion carries mandates checked before any money moves.
//!
//! **Carts** (`/carts`): create, retrieve, replace (`PUT`), cancel: items
//! and estimated totals before a checkout, with messages for items that
//! are not sold here rather than silently dropping them.
//!
//! **Orders**: made when a payment is verified, in ACP's order shape (line
//! items with ordered, current and fulfilled quantities, fulfillments,
//! adjustments, totals), sent to the agent's platform as `order_create`
//! and `order_update` webhooks ([`super::webhooks`]), each at a public
//! page (`/orders/{id}`). The merchant's operator fulfills and refunds
//! through an admin API (`/admin/…`, its own token): a refund of a ledger
//! payment is a transfer back to the payer on the ledger; of a card
//! payment, a refund at the PSP.
//!
//! **State** ([`super::state`]): sessions, carts, orders, idempotency keys,
//! AP2's payment history and the webhook outbox are written down after
//! every change. A restart takes back each open session's quote, so a
//! payment signed before it still settles; a ledger payment that was being
//! settled when the merchant stopped is finished from its receipt once the
//! merchant is back, or let go if it never executed.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sion_core::hash::Hash32;
use sion_core::ids::{AccountId, AssetId, TxDigest};
use sion_core::Amount;
use sion_exec::{ExecStatus, SignedTransaction, TxKind};

use crate::ledger::TxState;

use super::card::{self, CardHandler, ChargeRequest, Declined};
use super::http::{self, Gate, Keys, Request, Response, Service};
use super::state::Store;
use super::webhooks::{self, Delivery, Hook};
use super::{
    AP2_EXTENSION, API_VERSION, HANDLER, HANDLER_ID, HANDLER_SPEC, HANDLER_VERSION, INSTRUMENT,
    MICROS_PER_CENT,
};
use crate::ap2::{self, Refusal, SigningKey, Trust};
use crate::error::CliError;
use crate::x402::{Facilitator, PaymentPayload, PaymentRequirements, ResourceInfo, X402_VERSION};

/// How long a session's quote holds, in seconds.
pub const QUOTE_FOR: u64 = 30 * 60;

/// How long a cart lives after its last change, in seconds.
pub const CART_FOR: u64 = 24 * 3600;

/// How long a payment the ledger has never seen is waited for, in seconds.
const NEVER_SEEN_AFTER: u64 = 300;

/// The most sessions and carts kept at once; finished ones go first.
const MAX_KEPT: usize = 10_000;

// ----- Catalog ------------------------------------------------------------

/// Something the merchant sells.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    /// Its id, which a buyer names.
    pub id: String,
    /// What it is called.
    pub name: String,
    /// What it is.
    pub description: String,
    /// Its price, in micro-USD: a whole number of cents.
    pub price: Amount,
}

/// What the merchant sells.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Catalog {
    /// The offers, by id.
    pub offers: BTreeMap<String, Offer>,
}

impl Catalog {
    /// Reads `{"offers": [{"id", "name", "description", "price_usd"}]}`.
    pub fn from_json(value: &Value) -> Result<Catalog, CliError> {
        let mut offers = BTreeMap::new();
        for offer in value["offers"].as_array().into_iter().flatten() {
            let id = offer["id"]
                .as_str()
                .filter(|id| !id.is_empty())
                .ok_or_else(|| CliError::Usage("every offer has an id".into()))?;
            let price = offer["price_usd"]
                .as_str()
                .ok_or_else(|| CliError::Usage(format!("{id}: price_usd is a decimal string")))
                .and_then(crate::agents::parse_amount)?;
            if price.0 == 0 || price.0 % MICROS_PER_CENT != 0 {
                return Err(CliError::Usage(format!(
                    "{id}: a price is a whole number of cents, more than none"
                )));
            }
            offers.insert(
                id.to_string(),
                Offer {
                    id: id.to_string(),
                    name: offer["name"].as_str().unwrap_or(id).to_string(),
                    description: offer["description"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    price,
                },
            );
        }
        if offers.is_empty() {
            return Err(CliError::Usage("a catalog offers something".into()));
        }
        Ok(Catalog { offers })
    }
}

// ----- What is kept -------------------------------------------------------

/// A line of a session, a cart or an order: an offer at the price it had.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Line {
    /// The offer's id.
    pub offer: String,
    /// Its name.
    pub name: String,
    /// What it is.
    pub description: String,
    /// Its price, in cents.
    pub unit: u64,
    /// How many.
    pub quantity: u64,
}

impl Line {
    fn total(&self) -> u64 {
        self.unit * self.quantity
    }
}

fn total_of(lines: &[Line]) -> u64 {
    lines.iter().map(Line::total).sum()
}

/// A checkout session's status, as ACP names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// It can be paid.
    ReadyForPayment,
    /// A card payment waits for the buyer to be authenticated (3-D Secure).
    AuthenticationRequired,
    /// Its payment is being settled.
    CompleteInProgress,
    /// Paid; it has an order.
    Completed,
    /// Canceled.
    Canceled,
    /// Its quote expired.
    Expired,
}

impl Status {
    /// Its name in ACP.
    pub fn name(self) -> &'static str {
        match self {
            Status::ReadyForPayment => "ready_for_payment",
            Status::AuthenticationRequired => "authentication_required",
            Status::CompleteInProgress => "complete_in_progress",
            Status::Completed => "completed",
            Status::Canceled => "canceled",
            Status::Expired => "expired",
        }
    }
}

/// A ledger payment being settled: enough to finish it after a restart.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Pending {
    digest: String,
    payer: String,
    since: u64,
    ap2: Option<Value>,
}

/// What a completion's mandates were counted as when they were checked:
/// the open checkout mandates it used first, and the payment it added to
/// its open payment mandate's history. Taken back if the payment fails.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Counted {
    used: Vec<String>,
    paid: Option<(String, ap2::Paid)>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Checkout {
    id: String,
    status: Status,
    lines: Vec<Line>,
    buyer: Option<Value>,
    requirements: Option<PaymentRequirements>,
    created: u64,
    updated: u64,
    expires: u64,
    checkout_jwt: Option<String>,
    nonce: String,
    receipts: BTreeMap<String, String>,
    messages: Vec<Value>,
    order: Option<String>,
    pending: Option<Pending>,
    /// What its mandates were counted as, while it is being paid.
    #[serde(default)]
    counting: Option<Counted>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Cart {
    id: String,
    lines: Vec<Line>,
    buyer: Option<Value>,
    messages: Vec<Value>,
    updated: u64,
    expires: u64,
}

/// How an order was paid.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "rail", rename_all = "snake_case")]
pub enum Paid {
    /// On the ledger: a transaction whose receipt verified.
    Ledger {
        /// The payer's account.
        payer: String,
        /// The transaction's digest.
        transaction: String,
        /// The chain.
        network: String,
        /// The Pulse it executed in.
        pulse: Value,
    },
    /// By card, as the PSP says.
    Card {
        /// The PSP.
        psp: String,
        /// The PSP's id for the charge.
        charge: String,
    },
}

/// An order, as the merchant keeps it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Order {
    /// Its id.
    pub id: String,
    /// The session it came from.
    pub session: String,
    /// What was bought.
    pub lines: Vec<Line>,
    /// How it was paid.
    pub paid: Paid,
    /// How it was delivered (ACP `Fulfillment`s).
    pub fulfillments: Vec<Value>,
    /// What changed after (ACP `Adjustment`s: refunds).
    pub adjustments: Vec<Value>,
    /// How much of it was refunded, in cents.
    pub refunded: u64,
    /// When it was made.
    pub created: u64,
    /// The buyer, as given.
    pub buyer: Option<Value>,
    /// The AP2 mandates it was paid under.
    pub ap2: Option<Value>,
}

#[derive(Default, Serialize, Deserialize)]
struct Book {
    #[serde(default)]
    sessions: BTreeMap<String, Checkout>,
    #[serde(default)]
    carts: BTreeMap<String, Cart>,
    #[serde(default)]
    orders: BTreeMap<String, Order>,
    #[serde(default)]
    keys: Keys,
    /// Payments made under each open AP2 payment mandate.
    #[serde(default)]
    ap2_payments: BTreeMap<String, Vec<ap2::Paid>>,
    /// Open AP2 checkout mandates an order was made under.
    #[serde(default)]
    ap2_used: BTreeSet<String>,
    #[serde(default)]
    outbox: Vec<Delivery>,
}

/// AP2 as a merchant takes it.
struct Ap2 {
    key: SigningKey,
    trust: Trust,
    required: bool,
}

/// The mandates a completion came with, approved: the checkout's, and the
/// payment's if one came.
struct Approved {
    checkout: ap2::CheckoutApproved,
    payment: Option<ap2::PaymentApproved>,
}

/// Refunds on the ledger: the merchant's wallet, and a session to sign
/// from it.
struct Refunder<'l> {
    session: crate::session::Session<'l>,
    wallet: String,
}

/// A Siontology merchant that speaks ACP.
pub struct Merchant<'l> {
    facilitator: Facilitator<'l>,
    pay_to: AccountId,
    name: String,
    base_url: String,
    catalog: Catalog,
    gate: Gate,
    admin: Option<String>,
    ap2: Option<Ap2>,
    card: Option<CardHandler>,
    hook: Option<Hook>,
    refunder: Option<Mutex<Refunder<'l>>>,
    book: Mutex<Book>,
    /// Sessions whose completion runs now, in this process.
    running: Mutex<BTreeSet<String>>,
    store: Option<Store>,
    outbound: sion_http::Client,
    orders_log: Option<PathBuf>,
}

impl std::fmt::Debug for Merchant<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Merchant({}, {})", self.name, self.facilitator.network())
    }
}

fn message(kind: &str, code: &str, param: Option<&str>, content: String) -> Value {
    let mut message = json!({
        "type": kind,
        "code": code,
        "content_type": "plain",
        "content": content,
    });
    if let Some(param) = param {
        message["param"] = json!(param);
    }
    message
}

impl<'l> Merchant<'l> {
    /// A merchant selling `catalog`, paid to `pay_to` and settled by
    /// `facilitator`, served at `base_url` to `clients`.
    pub fn new(
        facilitator: Facilitator<'l>,
        pay_to: AccountId,
        name: &str,
        base_url: &str,
        catalog: Catalog,
        clients: Vec<http::Client>,
    ) -> Result<Self, CliError> {
        Ok(Merchant {
            facilitator,
            pay_to,
            name: name.to_string(),
            base_url: base_url.trim_end_matches('/').to_string(),
            catalog,
            gate: Gate::new(clients, &[API_VERSION])?,
            admin: None,
            ap2: None,
            card: None,
            hook: None,
            refunder: None,
            book: Mutex::new(Book::default()),
            running: Mutex::new(BTreeSet::new()),
            store: None,
            outbound: http::outbound(true)?,
            orders_log: None,
        })
    }

    /// Also takes AP2 checkout and payment mandates whose roots `trust`
    /// signed, signing checkouts and receipts with `key`; `required`, a
    /// session completes only with a checkout mandate.
    pub fn with_ap2(mut self, key: SigningKey, trust: Trust, required: bool) -> Self {
        self.ap2 = Some(Ap2 {
            key,
            trust,
            required,
        });
        self
    }

    /// Also takes delegated cards, charged by `handler`'s PSP.
    pub fn with_card(mut self, handler: CardHandler) -> Self {
        self.card = Some(handler);
        self
    }

    /// Sends order webhooks with `hook`.
    pub fn with_webhooks(mut self, hook: Hook) -> Self {
        self.hook = Some(hook);
        self
    }

    /// Serves the admin API to the holder of `token` (16 characters or
    /// more).
    pub fn with_admin(mut self, token: &str) -> Result<Self, CliError> {
        if token.len() < 16 {
            return Err(CliError::Usage(
                "the admin token has at least 16 characters".into(),
            ));
        }
        self.admin = Some(token.to_string());
        Ok(self)
    }

    /// Refunds ledger payments from `wallet` (the payee's), signed in
    /// `session`.
    pub fn with_refunds(mut self, session: crate::session::Session<'l>, wallet: &str) -> Self {
        self.refunder = Some(Mutex::new(Refunder {
            session,
            wallet: wallet.to_string(),
        }));
        self
    }

    /// Appends each order to `path`, one JSON line each (an audit log; the
    /// orders themselves are in the state).
    pub fn with_orders(mut self, path: PathBuf) -> Self {
        self.orders_log = Some(path);
        self
    }

    /// Keeps its state in `store`, taking back what is there: sessions,
    /// carts, orders, keys, AP2's history and the outbox. Each open
    /// session's quote is restored, so payments signed before a restart
    /// still settle.
    pub fn with_state(mut self, store: Store) -> Result<Self, CliError> {
        let mut book: Book = store.load()?;
        http::release_running(&mut book.keys);
        let now = http::now();
        let mut interrupted = Vec::new();
        for session in book.sessions.values_mut() {
            // Stopped while a card was being charged (or mandates checked):
            // payable again. Its charge's idempotency key is the same on a
            // retry, so the PSP answers it as it did, and charges once.
            if session.status == Status::CompleteInProgress && session.pending.is_none() {
                session.status = Status::ReadyForPayment;
                session.messages = vec![message(
                    "warning",
                    "interrupted",
                    None,
                    "the merchant stopped while this session was being paid: complete it again with the same payment".into(),
                )];
                if let Some(counted) = session.counting.take() {
                    interrupted.push(counted);
                }
            }
            let open = matches!(
                session.status,
                Status::ReadyForPayment | Status::AuthenticationRequired
            );
            if let (true, Some(requirements)) = (open, &session.requirements) {
                // Only a quote for this merchant and this session's total is
                // taken back, whatever the file says.
                let total = u128::from(total_of(&session.lines)) * MICROS_PER_CENT;
                let ours = requirements.pay_to == self.pay_to.to_string()
                    && requirements.amount == total.to_string();
                if now < session.expires
                    && (!ours
                        || self
                            .facilitator
                            .restore(&resource_of(&session.id), requirements, session.expires)
                            .is_err())
                {
                    // Another chain's, or another payee's: priced again when
                    // next touched.
                    session.requirements = None;
                }
            }
        }
        for counted in &interrupted {
            Self::uncount(&mut book, counted);
        }
        *self.book.get_mut().expect("book lock") = book;
        self.store = Some(store);
        Ok(self)
    }

    /// The key a buyer checks this merchant's checkouts and receipts by, if
    /// it takes AP2.
    pub fn ap2_key(&self) -> Option<ap2::PublicKey> {
        self.ap2.as_ref().map(|a| a.key.public())
    }

    /// The merchant as AP2 names it.
    pub fn identity(&self) -> Value {
        json!({"id": self.pay_to.to_string(), "name": self.name, "website": self.base_url})
    }

    /// The discovery document (`/.well-known/acp.json`).
    pub fn discovery(&self) -> Value {
        let mut capabilities = json!({
            "services": ["checkout", "orders", "carts"],
            "supported_currencies": ["usd"],
        });
        if self.ap2.is_some() {
            capabilities["extensions"] = json!([{
                "name": AP2_EXTENSION,
                "spec": HANDLER_SPEC,
                "schema": "https://siontology.org/docs/agent/commerce/ap2.schema.json",
            }]);
        }
        json!({
            "protocol": {
                "name": "acp",
                "version": API_VERSION,
                "supported_versions": [API_VERSION],
                "documentation_url": HANDLER_SPEC,
            },
            "api_base_url": self.base_url,
            "transports": ["rest"],
            "capabilities": capabilities,
        })
    }

    /// Orders, newest first (for the operator).
    pub fn orders(&self) -> Vec<Value> {
        let book = self.book.lock().expect("book lock");
        let mut orders: Vec<&Order> = book.orders.values().collect();
        orders.sort_by_key(|o| std::cmp::Reverse(o.created));
        orders.into_iter().map(|o| self.order_view(o)).collect()
    }

    /// The webhook outbox (for the operator).
    pub fn outbox(&self) -> Vec<Delivery> {
        self.book.lock().expect("book lock").outbox.clone()
    }

    fn save(&self, book: &Book) {
        if let Some(store) = &self.store {
            if let Err(e) = store.save(book) {
                eprintln!("warning: the merchant's state was not saved: {e}");
            }
        }
    }

    fn lines_of(&self, items: &Value, strict: bool) -> Result<(Vec<Line>, Vec<Value>), Response> {
        let items = items.as_array().filter(|i| !i.is_empty()).ok_or_else(|| {
            http::invalid(
                "missing",
                "line_items lists what is bought",
                Some("$.line_items"),
            )
        })?;
        let mut lines: Vec<Line> = Vec::new();
        let mut messages = Vec::new();
        for (index, item) in items.iter().enumerate() {
            let id = item["id"].as_str().unwrap_or_default();
            let param = format!("$.line_items[{index}].id");
            let Some(offer) = self.catalog.offers.get(id) else {
                if strict {
                    return Err(http::invalid(
                        "invalid_item_id",
                        &format!("The item ID '{id}' does not exist."),
                        Some(&param),
                    ));
                }
                messages.push(message(
                    "error",
                    "not_found",
                    Some(&param),
                    format!("The item ID '{id}' is not sold here."),
                ));
                continue;
            };
            // ACP's checkout Item has no quantity (an item is named once
            // for each one bought); its cart examples give one: both are
            // taken.
            let quantity = match item.get("quantity") {
                None => 1,
                Some(q) => match q.as_u64() {
                    Some(q) if (1..=10_000).contains(&q) => q,
                    _ => {
                        return Err(http::invalid(
                            "invalid",
                            "a quantity is 1 to 10000",
                            Some(&format!("$.line_items[{index}].quantity")),
                        ))
                    }
                },
            };
            match lines.iter_mut().find(|l| l.offer == offer.id) {
                Some(line) => line.quantity += quantity,
                None => lines.push(Line {
                    offer: offer.id.clone(),
                    name: offer.name.clone(),
                    description: offer.description.clone(),
                    unit: (offer.price.0 / MICROS_PER_CENT) as u64,
                    quantity,
                }),
            }
        }
        Ok((lines, messages))
    }

    /// Prices `session` afresh: a new quote, a new nonce and a new AP2
    /// checkout.
    fn quote(&self, session: &mut Checkout) {
        let total = Amount(u128::from(total_of(&session.lines)) * MICROS_PER_CENT);
        let resource = ResourceInfo {
            url: resource_of(&session.id),
            description: Some(format!("checkout session {}", session.id)),
            mime_type: Some("application/json".into()),
        };
        let quoted =
            self.facilitator
                .quote(resource, self.pay_to, total, Duration::from_secs(QUOTE_FOR));
        session.requirements = quoted.accepts.into_iter().next();
        let now = http::now();
        session.updated = now;
        session.expires = now + QUOTE_FOR;
        session.nonce = http::random_hex(16);
        if let Some(ap2) = &self.ap2 {
            session.checkout_jwt = Some(ap2::sign_jws(
                &json!({"typ": "JWT", "kid": "merchant"}),
                &self.checkout_of(session),
                &ap2.key,
            ));
        }
    }

    /// The checkout an AP2 mandate approves, in UCP's shape.
    fn checkout_of(&self, session: &Checkout) -> Value {
        let lines: Vec<Value> = session
            .lines
            .iter()
            .enumerate()
            .map(|(index, line)| {
                json!({
                    "id": format!("li_{index}"),
                    "item": {"id": line.offer, "title": line.name, "price": line.unit},
                    "quantity": line.quantity,
                    "totals": [
                        {"type": "subtotal", "amount": line.total()},
                        {"type": "total", "amount": line.total()},
                    ],
                })
            })
            .collect();
        let total = total_of(&session.lines);
        json!({
            "id": session.id,
            "merchant": self.identity(),
            "line_items": lines,
            "status": "ready_for_complete",
            "currency": "USD",
            // UCP's checkout requires its legal links: this merchant states
            // none.
            "links": [],
            "totals": [
                {"type": "subtotal", "amount": total},
                {"type": "total", "amount": total},
            ],
            "iat": session.updated,
            "nonce": session.nonce,
        })
    }

    fn line_views(lines: &[Line]) -> Vec<Value> {
        lines
            .iter()
            .enumerate()
            .map(|(index, line)| {
                json!({
                    "id": format!("li_{index}"),
                    "item": {"id": line.offer},
                    "quantity": line.quantity,
                    "name": line.name,
                    "description": line.description,
                    "unit_amount": line.unit,
                    "totals": [
                        {"type": "items_base_amount", "display_text": "Base Amount", "amount": line.total()},
                        {"type": "subtotal", "display_text": "Subtotal", "amount": line.total()},
                        {"type": "total", "display_text": "Total", "amount": line.total()},
                    ],
                })
            })
            .collect()
    }

    fn totals(total: u64) -> Value {
        json!([
            {"type": "items_base_amount", "display_text": "Item(s) total", "amount": total},
            {"type": "subtotal", "display_text": "Subtotal", "amount": total},
            {"type": "total", "display_text": "Total", "amount": total},
        ])
    }

    fn ledger_handler(&self, requirements: &PaymentRequirements) -> Value {
        json!({
            "id": HANDLER_ID,
            "name": HANDLER,
            "display_name": "Stablecoin USD on Siontology",
            "version": HANDLER_VERSION,
            "spec": HANDLER_SPEC,
            "requires_delegate_payment": false,
            "requires_pci_compliance": false,
            "psp": "siontology",
            "config_schema": "https://siontology.org/docs/agent/commerce/config.schema.json",
            "instrument_schemas": ["https://siontology.org/docs/agent/commerce/instrument.schema.json"],
            "config": {
                "network": requirements.network,
                "chain": requirements.extra["chain"],
                "asset": "USD",
                "micros_per_cent": MICROS_PER_CENT as u64,
                "pay_to": self.pay_to.to_string(),
                "requirements": requirements,
            },
            "display_order": 0,
        })
    }

    fn status_of(session: &Checkout, now: u64) -> Status {
        match session.status {
            Status::ReadyForPayment | Status::AuthenticationRequired if now > session.expires => {
                Status::Expired
            }
            status => status,
        }
    }

    /// A session as ACP answers it.
    fn view(&self, session: &Checkout, book: &Book) -> Value {
        let status = Self::status_of(session, http::now());
        let open = matches!(
            status,
            Status::ReadyForPayment | Status::AuthenticationRequired
        );
        let mut handlers = Vec::new();
        if open {
            if let Some(requirements) = &session.requirements {
                handlers.push(self.ledger_handler(requirements));
            }
            if let Some(card) = &self.card {
                handlers.push(card.declaration());
            }
        }
        let mut capabilities = json!({"payment": {"handlers": handlers}});
        if self.ap2.is_some() {
            capabilities["extensions"] = json!([{
                "name": AP2_EXTENSION,
                "extends": ["$.CheckoutSession.ap2", "$.CheckoutSessionCompleteRequest.ap2"],
                "schema": "https://siontology.org/docs/agent/commerce/ap2.schema.json",
            }]);
        }
        let item_ids: Vec<&str> = session.lines.iter().map(|l| l.offer.as_str()).collect();
        let mut view = json!({
            "id": session.id,
            "protocol": {"version": API_VERSION},
            "capabilities": capabilities,
            "status": status.name(),
            "currency": "usd",
            "line_items": Self::line_views(&session.lines),
            "fulfillment_options": [{
                "type": "digital",
                "id": "agent_delivery",
                "title": "Delivered by the agent",
                "totals": [{"type": "fulfillment", "display_text": "Delivery", "amount": 0}],
            }],
            "selected_fulfillment_options": [{"type": "digital", "option_id": "agent_delivery", "item_ids": item_ids}],
            "totals": Self::totals(total_of(&session.lines)),
            "messages": session.messages,
            "links": [],
            "created_at": http::rfc3339(session.created),
            "updated_at": http::rfc3339(session.updated),
            "expires_at": http::rfc3339(session.expires),
        });
        if let Some(metadata) = self.card.as_ref().and_then(|c| c.three_ds.as_ref()) {
            view["authentication_metadata"] = metadata.clone();
        }
        if let Some(buyer) = &session.buyer {
            view["buyer"] = buyer.clone();
        }
        if let Some(order) = session.order.as_ref().and_then(|id| book.orders.get(id)) {
            view["order"] = self.order_view(order);
        }
        if self.ap2.is_some() {
            let mut ap2 = json!({
                "checkout_jwt": session.checkout_jwt,
                "audience": self.pay_to.to_string(),
                "nonce": session.nonce,
                "merchant": self.identity(),
            });
            for (name, receipt) in &session.receipts {
                ap2[name] = json!(receipt);
            }
            view["ap2"] = ap2;
        }
        view
    }

    /// An order as ACP has it.
    fn order_view(&self, order: &Order) -> Value {
        let mut fulfilled: BTreeMap<String, u64> = BTreeMap::new();
        for fulfillment in &order.fulfillments {
            for line in fulfillment["line_items"].as_array().into_iter().flatten() {
                *fulfilled
                    .entry(line["id"].as_str().unwrap_or_default().to_string())
                    .or_default() += line["quantity"].as_u64().unwrap_or(0);
            }
        }
        let lines: Vec<Value> = order
            .lines
            .iter()
            .enumerate()
            .map(|(index, line)| {
                let id = format!("li_{index}");
                let done = fulfilled.get(&id).copied().unwrap_or(0).min(line.quantity);
                let status = if done == line.quantity {
                    "fulfilled"
                } else if done > 0 {
                    "partial"
                } else {
                    "processing"
                };
                json!({
                    "id": id,
                    "title": line.name,
                    "product_id": line.offer,
                    "quantity": {"ordered": line.quantity, "current": line.quantity, "fulfilled": done},
                    "unit_price": line.unit,
                    "subtotal": line.total(),
                    "status": status,
                })
            })
            .collect();
        let delivered = !order.lines.is_empty()
            && order.lines.iter().enumerate().all(|(index, line)| {
                fulfilled.get(&format!("li_{index}")).copied().unwrap_or(0) >= line.quantity
            });
        let total = total_of(&order.lines);
        let mut totals = vec![
            json!({"type": "subtotal", "display_text": "Subtotal", "amount": total}),
            json!({"type": "total", "display_text": "Total", "amount": total}),
        ];
        if order.refunded > 0 {
            totals.push(json!({"type": "amount_refunded", "display_text": "Refunded", "amount": order.refunded}));
        }
        json!({
            "type": "order",
            "id": order.id,
            "checkout_session_id": order.session,
            "permalink_url": format!("{}/orders/{}", self.base_url, order.id),
            "status": if delivered { "completed" } else { "confirmed" },
            "line_items": lines,
            "fulfillments": order.fulfillments,
            "adjustments": order.adjustments,
            "totals": totals,
        })
    }

    fn webhook(&self, book: &mut Book, kind: &str, order: &Order) {
        if self.hook.is_some() {
            let event = webhooks::event(kind, &self.order_view(order));
            book.outbox.push(Delivery::new(&event, http::now()));
        }
    }

    fn create(&self, body: &Value) -> Response {
        let currency = body["currency"].as_str().unwrap_or_default();
        if !currency.eq_ignore_ascii_case("usd") {
            return http::invalid("invalid", "this merchant sells in usd", Some("$.currency"));
        }
        let lines = match self.lines_of(&body["line_items"], true) {
            Ok((lines, _)) => lines,
            Err(refused) => return refused,
        };
        let now = http::now();
        let mut session = Checkout {
            id: http::fresh_id("cs"),
            status: Status::ReadyForPayment,
            lines,
            buyer: body.get("buyer").filter(|b| !b.is_null()).cloned(),
            requirements: None,
            created: now,
            updated: now,
            expires: now,
            checkout_jwt: None,
            nonce: String::new(),
            receipts: BTreeMap::new(),
            messages: Vec::new(),
            order: None,
            pending: None,
            counting: None,
        };
        self.quote(&mut session);
        let mut book = self.book.lock().expect("book lock");
        if book.sessions.len() >= MAX_KEPT {
            let oldest = book
                .sessions
                .values()
                .filter(|s| s.status != Status::CompleteInProgress)
                .min_by_key(|s| (matches!(s.status, Status::ReadyForPayment), s.updated))
                .map(|s| s.id.clone());
            if let Some(oldest) = oldest {
                book.sessions.remove(&oldest);
            }
        }
        let view = self.view(&session, &book);
        book.sessions.insert(session.id.clone(), session);
        self.save(&book);
        Response::json(201, view)
    }

    fn update(&self, id: &str, body: &Value) -> Response {
        let lines = match body.get("line_items") {
            Some(items) => match self.lines_of(items, true) {
                Ok((lines, _)) => Some(lines),
                Err(refused) => return refused,
            },
            None => None,
        };
        let mut book = self.book.lock().expect("book lock");
        let Some(mut session) = book.sessions.get(id).cloned() else {
            return http::not_found("no such checkout session");
        };
        if !matches!(
            session.status,
            Status::ReadyForPayment | Status::AuthenticationRequired | Status::Expired
        ) {
            return http::invalid(
                "invalid",
                &format!("the session is {}", session.status.name()),
                None,
            );
        }
        if let Some(lines) = lines {
            session.lines = lines;
        }
        if let Some(buyer) = body.get("buyer") {
            session.buyer = (!buyer.is_null()).then(|| buyer.clone());
        }
        session.messages.clear();
        session.status = Status::ReadyForPayment;
        self.quote(&mut session);
        let view = self.view(&session, &book);
        book.sessions.insert(session.id.clone(), session);
        self.save(&book);
        Response::json(200, view)
    }

    fn get(&self, id: &str) -> Response {
        let book = self.book.lock().expect("book lock");
        match book.sessions.get(id) {
            Some(session) => Response::json(200, self.view(session, &book)),
            None => http::not_found("no such checkout session"),
        }
    }

    fn cancel(&self, id: &str) -> Response {
        let mut book = self.book.lock().expect("book lock");
        let Some(mut session) = book.sessions.get(id).cloned() else {
            return http::not_found("no such checkout session");
        };
        if matches!(
            session.status,
            Status::Completed | Status::Canceled | Status::CompleteInProgress
        ) {
            return http::error(
                405,
                "invalid_request",
                "invalid",
                &format!("the session is already {}", session.status.name()),
                None,
            );
        }
        session.status = Status::Canceled;
        session.updated = http::now();
        let view = self.view(&session, &book);
        book.sessions.insert(session.id.clone(), session);
        self.save(&book);
        Response::json(200, view)
    }

    // ----- Completion ----------------------------------------------------

    /// Checks the AP2 mandates `presented`, if any, and counts them: an open
    /// checkout mandate is used once (unless the payment mandate with it
    /// recurs), and a payment is added to its open payment mandate's
    /// history, which its budget and recurrence are checked against. The
    /// check and the count are made under one lock, so two completions
    /// under the same mandates are counted one after the other.
    fn check_mandates(
        &self,
        session: &Checkout,
        presented: Option<&Value>,
    ) -> Result<Option<Approved>, (String, Refusal)> {
        let Some(ap2) = &self.ap2 else {
            return Ok(None);
        };
        let checkout_jwt = session.checkout_jwt.as_deref().unwrap_or_default();
        let Some(chain) = presented.and_then(|p| p["checkout_mandate"].as_str()) else {
            if ap2.required {
                return Err((
                    "checkout".into(),
                    Refusal::InvalidMandate(
                        "this merchant requires an AP2 checkout mandate".into(),
                    ),
                ));
            }
            return Ok(None);
        };
        let audience = self.pay_to.to_string();
        let now = http::now();
        let expect = ap2::Expect {
            audience: &audience,
            nonce: &session.nonce,
            now,
        };
        let checkout =
            ap2::verify_checkout(chain, &ap2.trust, expect, checkout_jwt, &ap2.key.public())
                .map_err(|refusal| ("checkout".to_string(), refusal))?;
        let payment_chain = presented.and_then(|p| p["payment_mandate"].as_str());
        // The payments made before under the open payment mandate are found
        // by its hash, known once the chain is read.
        let read = match payment_chain {
            Some(chain) => Some(
                ap2::verify_chain(chain, &ap2.trust, expect)
                    .map_err(|refusal| ("payment".to_string(), refusal))?,
            ),
            None => None,
        };
        let open_payment = read
            .as_ref()
            .and_then(|r| r.open().last().map(|(_, hash)| hash.to_string()));
        let recurring = read.as_ref().is_some_and(|r| {
            r.open().iter().any(|(open, _)| {
                open["constraints"]
                    .as_array()
                    .is_some_and(|all| all.iter().any(|c| c["type"] == "payment.agent_recurrence"))
            })
        });
        let amount = total_of(&session.lines);
        let identity = self.identity();
        let mut book = self.book.lock().expect("book lock");
        if !recurring
            && checkout
                .open
                .iter()
                .any(|open| book.ap2_used.contains(open))
        {
            return Err((
                "checkout".into(),
                Refusal::InvalidMandate("an order was already made under this open mandate".into()),
            ));
        }
        let payment = match payment_chain {
            None => None,
            Some(chain) => {
                let before: Vec<ap2::Paid> = open_payment
                    .as_ref()
                    .and_then(|open| book.ap2_payments.get(open).cloned())
                    .unwrap_or_default();
                let payment = ap2::Payment {
                    checkout_hash: &checkout.checkout_hash,
                    payee: &identity,
                    amount,
                    currency: "USD",
                    checkout_open: &checkout.open,
                    history: Some(&before),
                    now,
                };
                Some(
                    ap2::verify_payment(chain, &ap2.trust, expect, &payment)
                        .map_err(|refusal| ("payment".to_string(), refusal))?,
                )
            }
        };
        let mut counted = Counted::default();
        for open in &checkout.open {
            if book.ap2_used.insert(open.clone()) {
                counted.used.push(open.clone());
            }
        }
        if let Some(open) = payment.as_ref().and_then(|p| p.open.clone()) {
            let paid = ap2::Paid { at: now, amount };
            book.ap2_payments
                .entry(open.clone())
                .or_default()
                .push(paid);
            counted.paid = Some((open, paid));
        }
        if let Some(kept) = book.sessions.get_mut(&session.id) {
            kept.counting = Some(counted);
        }
        self.save(&book);
        Ok(Some(Approved { checkout, payment }))
    }

    /// Takes back what a failed payment's mandates were counted as.
    fn uncount(book: &mut Book, counted: &Counted) {
        for open in &counted.used {
            book.ap2_used.remove(open);
        }
        if let Some((open, paid)) = &counted.paid {
            if let Some(history) = book.ap2_payments.get_mut(open) {
                if let Some(at) = history.iter().rposition(|p| p == paid) {
                    history.remove(at);
                }
                if history.is_empty() {
                    book.ap2_payments.remove(open);
                }
            }
        }
    }

    /// Refuses a completion's mandate: the session stays payable, with the
    /// refusal as a message and its error receipt.
    fn refuse_mandate(
        &self,
        id: &str,
        which: &str,
        refusal: &Refusal,
        presented: Option<&Value>,
    ) -> Response {
        let mut book = self.book.lock().expect("book lock");
        let Some(mut session) = book.sessions.get(id).cloned() else {
            return http::not_found("no such checkout session");
        };
        if let (Some(ap2), Some(chain)) = (
            &self.ap2,
            presented.and_then(|p| p[format!("{which}_mandate")].as_str()),
        ) {
            let reference = ap2::split(chain)
                .ok()
                .and_then(|tokens| tokens.last().and_then(|t| t.sd_hash().ok()))
                .unwrap_or_default();
            let kind = if which == "checkout" {
                ap2::Kind::Checkout
            } else {
                ap2::Kind::Payment
            };
            session.receipts.insert(
                format!("{which}_receipt"),
                ap2::receipt(
                    kind,
                    &ap2.key,
                    &self.base_url,
                    &reference,
                    Err(refusal),
                    http::now(),
                ),
            );
        }
        session.messages = vec![message(
            "error",
            "invalid",
            Some(&format!("$.ap2.{which}_mandate")),
            format!("AP2 {which} mandate refused: {refusal}"),
        )];
        session.status = Status::ReadyForPayment;
        let view = self.view(&session, &book);
        book.sessions.insert(session.id.clone(), session);
        self.save(&book);
        Response::json(200, view)
    }

    fn complete(&self, id: &str, body: &Value) -> Response {
        let payment = &body["payment_data"];
        if payment.is_null() {
            return http::invalid(
                "missing",
                "payment_data is required",
                Some("$.payment_data"),
            );
        }
        let handler = payment["handler_id"].as_str().unwrap_or_default();
        let card_id = self.card.as_ref().map(|_| card::HANDLER_ID);
        if handler != HANDLER_ID && Some(handler) != card_id {
            let offered = match card_id {
                Some(card) => format!("{HANDLER_ID} or {card}"),
                None => HANDLER_ID.to_string(),
            };
            return http::invalid(
                "invalid",
                &format!("the handlers here are {offered}"),
                Some("$.payment_data.handler_id"),
            );
        }
        let instrument = &payment["instrument"];
        // Claim the session: it is now being completed, here.
        let session = {
            let mut book = self.book.lock().expect("book lock");
            let Some(session) = book.sessions.get_mut(id) else {
                return http::not_found("no such checkout session");
            };
            match Self::status_of(session, http::now()) {
                Status::ReadyForPayment | Status::AuthenticationRequired => {}
                Status::Expired => {
                    return http::invalid(
                        "expired",
                        "the session's quote expired: update it to price it again",
                        None,
                    )
                }
                other => {
                    return http::invalid(
                        "invalid",
                        &format!("the session is {}", other.name()),
                        None,
                    )
                }
            }
            if !self
                .running
                .lock()
                .expect("running lock")
                .insert(id.to_string())
            {
                let view = self.view(&book.sessions[id], &book);
                return Response::json(200, view);
            }
            let session = book.sessions.get_mut(id).expect("present");
            if let Some(buyer) = body.get("buyer").filter(|b| !b.is_null()) {
                session.buyer = Some(buyer.clone());
            }
            session.messages.clear();
            session.status = Status::CompleteInProgress;
            let session = session.clone();
            self.save(&book);
            session
        };
        let answer = self.complete_claimed(session, body, handler, instrument);
        self.running.lock().expect("running lock").remove(id);
        answer
    }

    fn complete_claimed(
        &self,
        session: Checkout,
        body: &Value,
        handler: &str,
        instrument: &Value,
    ) -> Response {
        let id = session.id.clone();
        // Mandates before money: nothing moves for a refused one.
        let mandates = match self.check_mandates(&session, body.get("ap2")) {
            Ok(mandates) => mandates,
            Err((which, refusal)) => {
                return self.refuse_mandate(&id, &which, &refusal, body.get("ap2"))
            }
        };
        if handler == HANDLER_ID {
            self.pay_on_ledger(session, instrument, mandates)
        } else {
            self.pay_by_card(session, body, instrument, mandates)
        }
    }

    fn back_to_payable(
        &self,
        id: &str,
        messages: Vec<Value>,
        status: Status,
        requote: bool,
    ) -> Response {
        let mut book = self.book.lock().expect("book lock");
        let Some(mut session) = book.sessions.get(id).cloned() else {
            return http::not_found("no such checkout session");
        };
        // Not paid: its mandates were not used.
        if let Some(counted) = session.counting.take() {
            Self::uncount(&mut book, &counted);
        }
        session.status = status;
        session.messages = messages;
        session.pending = None;
        if requote {
            self.quote(&mut session);
        }
        let view = self.view(&session, &book);
        book.sessions.insert(session.id.clone(), session);
        self.save(&book);
        Response::json(200, view)
    }

    fn pay_on_ledger(
        &self,
        session: Checkout,
        instrument: &Value,
        mandates: Option<Approved>,
    ) -> Response {
        let id = session.id.clone();
        // ACP's instrument carries its credential as `token`; this
        // handler's first version named it `transaction`, still taken.
        let credential = &instrument["credential"];
        let transaction = credential["token"]
            .as_str()
            .or_else(|| credential["transaction"].as_str())
            .filter(|_| instrument["type"] == INSTRUMENT);
        let Some(transaction) = transaction else {
            self.back_to_payable(&id, Vec::new(), Status::ReadyForPayment, false);
            return http::invalid(
                "invalid",
                &format!("the instrument is a {INSTRUMENT} whose credential carries the signed transaction"),
                Some("$.payment_data.instrument"),
            );
        };
        let Some(requirements) = session.requirements.clone() else {
            return self.back_to_payable(
                &id,
                vec![message(
                    "error",
                    "expired",
                    None,
                    "the session is priced again: pay against its new quote".into(),
                )],
                Status::ReadyForPayment,
                true,
            );
        };
        let Some(signed) = hex::decode(transaction)
            .ok()
            .and_then(|bytes| sion_core::encode::decode::<SignedTransaction>(&bytes).ok())
        else {
            self.back_to_payable(&id, Vec::new(), Status::ReadyForPayment, false);
            return http::invalid(
                "invalid",
                "the transaction does not decode",
                Some("$.payment_data.instrument.credential.transaction"),
            );
        };
        // Written down before it is submitted: a restart finishes it.
        let ap2_record = Self::ap2_record(&mandates);
        {
            let mut book = self.book.lock().expect("book lock");
            if let Some(kept) = book.sessions.get_mut(&id) {
                kept.pending = Some(Pending {
                    digest: signed.digest().to_string(),
                    payer: signed.tx.sender.to_string(),
                    since: http::now(),
                    ap2: ap2_record.clone(),
                });
            }
            self.save(&book);
        }
        let paid = PaymentPayload {
            x402_version: X402_VERSION,
            resource: Some(ResourceInfo {
                url: resource_of(&id),
                description: None,
                mime_type: None,
            }),
            accepted: requirements,
            payload: json!({"transaction": transaction}),
            extensions: None,
        };
        let settled = self.facilitator.settle(&resource_of(&id), &paid);
        if !settled.success {
            let why = settled.error_reason.unwrap_or_default();
            // Submitted, and not refused: it may yet execute, so it is left
            // for the recovery to finish from its receipt.
            let maybe_executing = !settled.transaction.is_empty()
                && !why.contains("refused")
                && !why.contains("not certified");
            if maybe_executing {
                let book = self.book.lock().expect("book lock");
                let view = self.view(&book.sessions[&id], &book);
                return Response::json(200, view);
            }
            return self.back_to_payable(
                &id,
                vec![{
                    let mut m = message(
                        "error",
                        "payment_declined",
                        Some("$.payment_data"),
                        format!("the payment did not settle: {why}"),
                    );
                    m["resolution"] = json!("recoverable");
                    m
                }],
                Status::ReadyForPayment,
                true,
            );
        }
        let pulse = settled
            .extensions
            .as_ref()
            .map(|e| e["sion"]["pulse"].clone())
            .unwrap_or(Value::Null);
        let paid = Paid::Ledger {
            payer: settled.payer.clone().unwrap_or_default(),
            transaction: settled.transaction.clone(),
            network: settled.network.clone(),
            pulse,
        };
        self.finish(&id, paid, ap2_record)
    }

    fn ap2_record(mandates: &Option<Approved>) -> Option<Value> {
        mandates.as_ref().map(|Approved { checkout, payment }| {
            json!({
                "checkout_mandate": checkout.reference,
                "checkout_open": checkout.open,
                "payment_mandate": payment.as_ref().map(|p| p.reference.clone()),
                "payment_open": payment.as_ref().and_then(|p| p.open.clone()),
            })
        })
    }

    fn pay_by_card(
        &self,
        session: Checkout,
        body: &Value,
        instrument: &Value,
        mandates: Option<Approved>,
    ) -> Response {
        let id = session.id.clone();
        let card = self.card.as_ref().expect("a card handler");
        let token = instrument["credential"]["token"]
            .as_str()
            .filter(|_| instrument["type"] == "card" && instrument["credential"]["type"] == "spt");
        let Some(token) = token else {
            self.back_to_payable(&id, Vec::new(), Status::ReadyForPayment, false);
            return http::invalid(
                "invalid",
                "a card instrument's credential is {type: spt, token}",
                Some("$.payment_data.instrument.credential"),
            );
        };
        let authentication = body.get("authentication_result").filter(|a| !a.is_null());
        if let Some(result) = authentication {
            if let Err(why) = card::check_authentication(result) {
                self.back_to_payable(&id, Vec::new(), Status::AuthenticationRequired, false);
                return http::invalid("invalid", &why, Some("$.authentication_result"));
            }
        }
        if card.three_ds.is_some() {
            let outcome = authentication.and_then(|a| a["outcome"].as_str());
            match outcome {
                Some(outcome) if card::ACCEPTED_OUTCOMES.contains(&outcome) => {}
                Some(outcome) => {
                    return self.back_to_payable(
                        &id,
                        vec![message("error", "payment_declined", Some("$.authentication_result"), format!("3-D Secure ended {outcome}: the card is not charged"))],
                        Status::ReadyForPayment,
                        false,
                    )
                }
                None => {
                    return self.back_to_payable(
                        &id,
                        vec![message("error", "requires_3ds", Some("$.authentication_result"), "authenticate the card with 3-D Secure (see authentication_metadata), then complete with authentication_result".into())],
                        Status::AuthenticationRequired,
                        false,
                    )
                }
            }
        }
        // One charge for one token and one authentication: a retry after
        // 3-D Secure is a new charge, not a replay of the refused one.
        let attempt = format!(
            "{token}|{}",
            authentication
                .map(super::signing::canonical)
                .unwrap_or_default()
        );
        let key = format!("{id}-{}", &ap2::hash(&attempt)[..16]);
        let charged = card.processor.charge(&ChargeRequest {
            token,
            amount: total_of(&session.lines),
            currency: "usd",
            checkout_session_id: &id,
            idempotency_key: &key,
            authentication,
        });
        match charged {
            Ok(charge) => {
                let paid = Paid::Card {
                    psp: card.processor.psp().to_string(),
                    charge: charge.id,
                };
                let record = Self::ap2_record(&mandates);
                self.finish(&id, paid, record)
            }
            Err(Declined::NeedsAuthentication(why)) => self.back_to_payable(
                &id,
                vec![message(
                    "error",
                    "requires_3ds",
                    Some("$.authentication_result"),
                    why,
                )],
                Status::AuthenticationRequired,
                false,
            ),
            Err(declined) => self.back_to_payable(
                &id,
                vec![{
                    let mut m = message(
                        "error",
                        "payment_declined",
                        Some("$.payment_data"),
                        declined.describe().to_string(),
                    );
                    m["resolution"] = json!("recoverable");
                    m
                }],
                Status::ReadyForPayment,
                false,
            ),
        }
    }

    /// Makes the order for a verified payment.
    fn finish(&self, id: &str, paid: Paid, ap2_record: Option<Value>) -> Response {
        let now = http::now();
        let mut book = self.book.lock().expect("book lock");
        let Some(mut session) = book.sessions.get(id).cloned() else {
            return http::not_found("no such checkout session");
        };
        let order = Order {
            id: http::fresh_id("ord"),
            session: id.to_string(),
            lines: session.lines.clone(),
            paid,
            fulfillments: Vec::new(),
            adjustments: Vec::new(),
            refunded: 0,
            created: now,
            buyer: session.buyer.clone(),
            ap2: ap2_record.clone(),
        };
        session.status = Status::Completed;
        session.updated = now;
        session.order = Some(order.id.clone());
        session.pending = None;
        // Its mandates' count stands: they were used.
        session.counting = None;
        session.messages.clear();
        let total = total_of(&session.lines);
        if let (Some(ap2), Some(record)) = (&self.ap2, &ap2_record) {
            if let Some(reference) = record["checkout_mandate"].as_str() {
                session.receipts.insert(
                    "checkout_receipt".into(),
                    ap2::receipt(
                        ap2::Kind::Checkout,
                        &ap2.key,
                        &self.base_url,
                        reference,
                        Ok(&json!({"order_id": order.id})),
                        now,
                    ),
                );
            }
            if let Some(reference) = record["payment_mandate"].as_str() {
                let (payment_id, confirmation) = match &order.paid {
                    Paid::Ledger {
                        transaction,
                        network,
                        pulse,
                        ..
                    } => (transaction.clone(), format!("{network}@{pulse}")),
                    Paid::Card { psp, charge } => (charge.clone(), psp.clone()),
                };
                session.receipts.insert(
                    "payment_receipt".into(),
                    ap2::receipt(
                        ap2::Kind::Payment,
                        &ap2.key,
                        &self.base_url,
                        reference,
                        Ok(&json!({
                            "payment_id": payment_id,
                            "psp_confirmation_id": payment_id,
                            "network_confirmation_id": confirmation,
                        })),
                        now,
                    ),
                );
            }
        }
        self.webhook(&mut book, "order_create", &order);
        if let Some(path) = &self.orders_log {
            let transaction = match &order.paid {
                Paid::Ledger { transaction, .. } => transaction.clone(),
                Paid::Card { charge, .. } => charge.clone(),
            };
            let record = json!({
                "order": self.order_view(&order),
                "paid": order.paid,
                "transaction": transaction,
                "total_cents": total,
                "buyer": order.buyer,
                "ap2": order.ap2,
                "at": http::rfc3339(now),
            });
            let written = path
                .parent()
                .filter(|dir| !dir.as_os_str().is_empty())
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| {
                    std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(path)
                })
                .and_then(|mut file| writeln!(file, "{record}"));
            if let Err(e) = written {
                eprintln!(
                    "warning: order {} was paid but not logged to {}: {e}",
                    order.id,
                    path.display()
                );
            }
        }
        book.orders.insert(order.id.clone(), order);
        let view = self.view(&session, &book);
        book.sessions.insert(session.id.clone(), session);
        self.save(&book);
        Response::json(200, view)
    }

    /// Finishes ledger payments left settling (the merchant stopped, or the
    /// ledger was slow): from the verified receipt if one executed; let go
    /// if the ledger refused it, or never saw it.
    fn recover(&self) {
        let pending: Vec<(String, Pending)> = {
            let book = self.book.lock().expect("book lock");
            let running = self.running.lock().expect("running lock");
            book.sessions
                .values()
                .filter(|s| s.status == Status::CompleteInProgress && !running.contains(&s.id))
                .filter_map(|s| s.pending.clone().map(|p| (s.id.clone(), p)))
                .collect()
        };
        for (id, pending) in pending {
            let Ok(digest) = Hash32::from_hex(&pending.digest).map(TxDigest::from_hash) else {
                continue;
            };
            let ledger = self.facilitator.ledger();
            let declined = |why: String| {
                let mut m = message("error", "payment_declined", Some("$.payment_data"), why);
                m["resolution"] = json!("recoverable");
                vec![m]
            };
            match ledger.tx(digest) {
                Ok(TxState::Executed(view)) if view.status == ExecStatus::Success => {
                    // Paid: the order is made once its receipt verifies.
                    match crate::verify::verify_receipt_of(
                        ledger,
                        Some(self.facilitator.chain()),
                        digest,
                    ) {
                        Ok(receipt) if receipt.effects.status == ExecStatus::Success => {
                            let paid = Paid::Ledger {
                                payer: pending.payer.clone(),
                                transaction: pending.digest.clone(),
                                network: self.facilitator.network(),
                                pulse: json!(receipt.pulse),
                            };
                            let _ = self.finish(&id, paid, pending.ap2.clone());
                        }
                        Ok(_) => {
                            let _ = self.back_to_payable(
                                &id,
                                declined(
                                    "the payment's certified receipt does not show success".into(),
                                ),
                                Status::ReadyForPayment,
                                true,
                            );
                        }
                        Err(_) => {} // Not certified yet.
                    }
                }
                Ok(TxState::Executed(view)) => {
                    let _ = self.back_to_payable(
                        &id,
                        declined(format!("the ledger refused the payment: {:?}", view.status)),
                        Status::ReadyForPayment,
                        true,
                    );
                }
                Ok(TxState::Rejected(why)) => {
                    let _ = self.back_to_payable(
                        &id,
                        declined(format!("the payment was not certified: {why}")),
                        Status::ReadyForPayment,
                        true,
                    );
                }
                Ok(TxState::Unknown) if http::now() > pending.since + NEVER_SEEN_AFTER => {
                    let _ = self.back_to_payable(
                        &id,
                        declined("the payment never reached the ledger".into()),
                        Status::ReadyForPayment,
                        true,
                    );
                }
                // Pending, not seen yet, or the ledger unreachable: later.
                _ => {}
            }
        }
    }

    /// Delivers due webhooks, in order.
    fn deliver(&self) {
        let Some(hook) = &self.hook else {
            return;
        };
        let now = http::now();
        // In order: an event still waiting to be retried holds back the ones
        // after it (one given up does not).
        let due: Vec<Delivery> = {
            let book = self.book.lock().expect("book lock");
            book.outbox
                .iter()
                .filter(|d| d.delivered_at.is_none() && !d.gave_up)
                .take_while(|d| d.due(now))
                .cloned()
                .collect()
        };
        for delivery in due {
            let sent = hook.send(&self.outbound, &delivery, http::now());
            let mut book = self.book.lock().expect("book lock");
            if let Some(kept) = book.outbox.iter_mut().find(|d| d.id == delivery.id) {
                match &sent {
                    Ok(()) => kept.delivered_at = Some(http::now()),
                    Err(why) => kept.failed(http::now(), why.clone()),
                }
            }
            // Delivered events go a day after, given up ones a week after
            // their last try (the operator sees them in /admin/outbox).
            let now = http::now();
            let (day, week) = (24 * 3600, 7 * 24 * 3600);
            book.outbox.retain(|d| match d.delivered_at {
                Some(at) => at + day > now,
                None => !d.gave_up || d.next_at + week > now,
            });
            self.save(&book);
            if sent.is_err() {
                // Later events wait: they are delivered in order.
                break;
            }
        }
    }

    // ----- Carts ---------------------------------------------------------

    fn cart_view(&self, cart: &Cart) -> Value {
        let mut view = json!({
            "id": cart.id,
            "line_items": Self::line_views(&cart.lines),
            "currency": "usd",
            "totals": [
                {"type": "subtotal", "display_text": "Subtotal", "amount": total_of(&cart.lines)},
                {"type": "total", "display_text": "Total", "amount": total_of(&cart.lines)},
            ],
            "messages": cart.messages,
            "expires_at": http::rfc3339(cart.expires),
        });
        if let Some(buyer) = &cart.buyer {
            view["buyer"] = buyer.clone();
        }
        view
    }

    fn cart_create(&self, body: &Value) -> Response {
        let (lines, messages) = match self.lines_of(&body["line_items"], false) {
            Ok(found) => found,
            Err(refused) => return refused,
        };
        if lines.is_empty() {
            return http::error(
                422,
                "invalid_request",
                "out_of_stock",
                "All requested items are currently unavailable.",
                None,
            );
        }
        let now = http::now();
        let cart = Cart {
            id: http::fresh_id("cart"),
            lines,
            buyer: body.get("buyer").filter(|b| !b.is_null()).cloned(),
            messages,
            updated: now,
            expires: now + CART_FOR,
        };
        let view = self.cart_view(&cart);
        let mut book = self.book.lock().expect("book lock");
        book.carts.retain(|_, c| c.expires > now);
        if book.carts.len() >= MAX_KEPT {
            if let Some(oldest) = book
                .carts
                .values()
                .min_by_key(|c| c.updated)
                .map(|c| c.id.clone())
            {
                book.carts.remove(&oldest);
            }
        }
        book.carts.insert(cart.id.clone(), cart);
        self.save(&book);
        Response::json(201, view)
    }

    fn cart_get(&self, id: &str) -> Response {
        let book = self.book.lock().expect("book lock");
        match book.carts.get(id).filter(|c| c.expires > http::now()) {
            Some(cart) => Response::json(200, self.cart_view(cart)),
            None => http::not_found("no such cart"),
        }
    }

    fn cart_replace(&self, id: &str, body: &Value) -> Response {
        let (lines, messages) = match self.lines_of(&body["line_items"], false) {
            Ok(found) => found,
            Err(refused) => return refused,
        };
        let now = http::now();
        let mut book = self.book.lock().expect("book lock");
        let Some(cart) = book.carts.get_mut(id).filter(|c| c.expires > now) else {
            return http::not_found("no such cart");
        };
        cart.lines = lines;
        cart.messages = messages;
        if let Some(buyer) = body.get("buyer") {
            cart.buyer = (!buyer.is_null()).then(|| buyer.clone());
        }
        cart.updated = now;
        cart.expires = now + CART_FOR;
        let view = self.cart_view(cart);
        self.save(&book);
        Response::json(200, view)
    }

    fn cart_cancel(&self, id: &str) -> Response {
        let mut book = self.book.lock().expect("book lock");
        let now = http::now();
        match book.carts.remove(id).filter(|c| c.expires > now) {
            Some(cart) => {
                let view = self.cart_view(&cart);
                self.save(&book);
                Response::json(200, view)
            }
            None => http::not_found("no such cart"),
        }
    }

    // ----- The operator's admin API --------------------------------------

    fn fulfill(&self, id: &str, body: &Value) -> Response {
        let mut book = self.book.lock().expect("book lock");
        let Some(mut order) = book.orders.get(id).cloned() else {
            return http::not_found("no such order");
        };
        let now = http::now();
        let lines: Vec<Value> = order
            .lines
            .iter()
            .enumerate()
            .map(|(index, line)| json!({"id": format!("li_{index}"), "quantity": line.quantity}))
            .collect();
        let mut delivery = json!({});
        for field in ["access_url", "license_key"] {
            if let Some(value) = body[field].as_str() {
                delivery[field] = json!(value);
            }
        }
        let mut event = json!({"id": http::fresh_id("fev"), "type": "delivered", "occurred_at": http::rfc3339(now)});
        if let Some(description) = body["description"].as_str() {
            event["description"] = json!(description);
        }
        order.fulfillments.push(json!({
            "id": http::fresh_id("ful"),
            "type": "digital",
            "status": "delivered",
            "line_items": lines,
            "digital_delivery": delivery,
            "events": [event],
        }));
        self.webhook(&mut book, "order_update", &order);
        let view = self.order_view(&order);
        book.orders.insert(order.id.clone(), order);
        self.save(&book);
        Response::json(200, view)
    }

    fn refund(&self, id: &str, body: &Value) -> Response {
        let order = {
            let book = self.book.lock().expect("book lock");
            match book.orders.get(id) {
                Some(order) => order.clone(),
                None => return http::not_found("no such order"),
            }
        };
        let left = total_of(&order.lines).saturating_sub(order.refunded);
        let amount = body["amount"].as_u64().unwrap_or(left);
        if amount == 0 || amount > left {
            return http::invalid(
                "invalid",
                &format!("a refund is 1 to {left} cents"),
                Some("$.amount"),
            );
        }
        let key = format!("refund-{}-{}", order.id, order.adjustments.len());
        let done = match &order.paid {
            Paid::Ledger { payer, .. } => {
                let Some(refunder) = &self.refunder else {
                    return http::error(
                        409,
                        "invalid_request",
                        "unsupported",
                        "this merchant was not given its wallet to refund from",
                        None,
                    );
                };
                let Ok(payer) = Hash32::from_hex(payer).map(AccountId::from_hash) else {
                    return http::error(
                        500,
                        "processing_error",
                        "invalid",
                        "the payer's account does not read",
                        None,
                    );
                };
                let mut refunder = refunder.lock().expect("refunder lock");
                let wallet = refunder.wallet.clone();
                let kind = TxKind::Transfer {
                    asset: AssetId::usd(),
                    outputs: vec![(payer, Amount(u128::from(amount) * MICROS_PER_CENT))],
                };
                let sent = refunder
                    .session
                    .submit(&wallet, kind, Vec::new())
                    .and_then(|digest| refunder.session.wait(digest).map(|view| (digest, view)));
                refunder.session.settle_all();
                match sent {
                    Ok((digest, view)) if view.status == ExecStatus::Success => {
                        Ok(format!("refunded on Siontology in transaction {digest}"))
                    }
                    Ok((_, view)) => Err(format!(
                        "the refund failed on the ledger: {:?}",
                        view.status
                    )),
                    Err(e) => Err(e.to_string()),
                }
            }
            Paid::Card { psp, charge } => {
                let card = self.card.as_ref().expect("a card order has a card handler");
                card.processor
                    .refund(charge, amount, &key)
                    .map(|refund| format!("refunded by {psp} ({refund})"))
            }
        };
        let description = match done {
            Ok(description) => description,
            Err(why) => return http::error(502, "processing_error", "refund_failed", &why, None),
        };
        let mut book = self.book.lock().expect("book lock");
        let Some(order) = book.orders.get_mut(id) else {
            return http::not_found("no such order");
        };
        let mut adjustment = json!({
            "id": http::fresh_id("adj"),
            "type": "refund",
            "occurred_at": http::rfc3339(http::now()),
            "status": "completed",
            "amount": amount,
            "currency": "usd",
            "description": description,
        });
        if let Some(reason) = body["reason"].as_str() {
            adjustment["reason"] = json!(reason);
        }
        order.adjustments.push(adjustment);
        order.refunded += amount;
        let order = order.clone();
        self.webhook(&mut book, "order_update", &order);
        self.save(&book);
        Response::json(200, self.order_view(&order))
    }

    fn admin(&self, request: &Request<'_>) -> Response {
        let presented = request
            .header("Authorization")
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(str::trim);
        let allowed = matches!((&self.admin, presented), (Some(admin), Some(p)) if http::same_secret(admin, p));
        if !allowed {
            return http::error(
                401,
                "invalid_request",
                "unauthorized",
                "the admin token is needed",
                None,
            );
        }
        let body = match http::body_of(request) {
            Ok(body) => body,
            Err(refused) => return refused,
        };
        let parts: Vec<&str> = request.path.split('/').filter(|p| !p.is_empty()).collect();
        match (request.method, parts.as_slice()) {
            ("GET", ["admin", "orders"]) => Response::json(200, json!({"orders": self.orders()})),
            ("GET", ["admin", "outbox"]) => {
                Response::json(200, json!({"deliveries": self.outbox()}))
            }
            ("POST", ["admin", "orders", id, "fulfill"]) => self.fulfill(id, &body),
            ("POST", ["admin", "orders", id, "refund"]) => self.refund(id, &body),
            _ => http::not_found("no such admin endpoint"),
        }
    }

    fn order_page(&self, id: &str) -> Response {
        let book = self.book.lock().expect("book lock");
        match book.orders.get(id) {
            Some(order) => {
                let receipts = book
                    .sessions
                    .get(&order.session)
                    .map(|s| json!(s.receipts))
                    .unwrap_or(Value::Null);
                let mut page = json!({
                    "order": self.order_view(order),
                    "merchant": self.identity(),
                    "receipts": receipts,
                });
                if let Paid::Ledger { transaction, .. } = &order.paid {
                    page["verify_with"] = json!(format!(
                        "sion receipt {transaction} --chain <the chain identity you pin>"
                    ));
                }
                Response::json(200, page)
            }
            None => http::not_found("no such order here"),
        }
    }

    /// Runs `then` under the request's idempotency key, which every `POST`
    /// carries.
    fn keyed(
        &self,
        request: &Request<'_>,
        client: &str,
        required: bool,
        then: impl FnOnce(&Value) -> Response,
    ) -> Response {
        let body = match http::body_of(request) {
            Ok(body) => body,
            Err(refused) => return refused,
        };
        let key = match http::key_of(request, required) {
            Ok(key) => key,
            Err(refused) => return refused,
        };
        let Some(key) = key else {
            return then(&body);
        };
        let slot = {
            let mut book = self.book.lock().expect("book lock");
            match http::claim(&mut book.keys, 422, client, request.path, key, &body) {
                Ok(slot) => slot,
                Err(answered) => return answered,
            }
        };
        let response = then(&body);
        let mut book = self.book.lock().expect("book lock");
        http::settle(&mut book.keys, slot, &response);
        self.save(&book);
        response
    }
}

fn resource_of(session: &str) -> String {
    format!("/checkout_sessions/{session}")
}

impl Service for Merchant<'_> {
    fn answer(&self, request: &Request<'_>) -> Response {
        let response = self.route(request);
        http::echo(request, response)
    }

    fn tick(&self) {
        self.recover();
        self.deliver();
    }
}

impl Merchant<'_> {
    fn route(&self, request: &Request<'_>) -> Response {
        if request.method == "GET" {
            if request.path == "/.well-known/acp.json" {
                return Response::json(200, self.discovery());
            }
            // An order's page: public, as a buyer's link is.
            if let Some(id) = request.path.strip_prefix("/orders/") {
                return self.order_page(id);
            }
        }
        if request.path.starts_with("/admin/") {
            return self.admin(request);
        }
        let client = match self.gate.admit(request) {
            Ok(client) => client.name.clone(),
            Err(refused) => return refused,
        };
        let parts: Vec<&str> = request.path.split('/').filter(|p| !p.is_empty()).collect();
        match (request.method, parts.as_slice()) {
            ("POST", ["checkout_sessions"]) => {
                self.keyed(request, &client, true, |body| self.create(body))
            }
            ("POST", ["checkout_sessions", id]) => {
                self.keyed(request, &client, true, |body| self.update(id, body))
            }
            ("GET", ["checkout_sessions", id]) => self.get(id),
            ("POST", ["checkout_sessions", id, "complete"]) => {
                self.keyed(request, &client, true, |body| self.complete(id, body))
            }
            ("POST", ["checkout_sessions", id, "cancel"]) => {
                self.keyed(request, &client, true, |_| self.cancel(id))
            }
            ("POST", ["carts"]) => {
                self.keyed(request, &client, true, |body| self.cart_create(body))
            }
            ("GET", ["carts", id]) => self.cart_get(id),
            ("PUT", ["carts", id]) => {
                self.keyed(request, &client, false, |body| self.cart_replace(id, body))
            }
            ("POST", ["carts", id, "cancel"]) => {
                self.keyed(request, &client, true, |_| self.cart_cancel(id))
            }
            _ => http::not_found("no such endpoint"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_catalog_prices_in_whole_cents() {
        let catalog = Catalog::from_json(&json!({"offers": [
            {"id": "report", "name": "A research report", "price_usd": "4.50"},
        ]}))
        .unwrap();
        assert_eq!(catalog.offers["report"].price, Amount(4_500_000));
        for bad in [
            json!({"offers": [{"id": "x", "price_usd": "0.001"}]}),
            json!({"offers": [{"id": "x", "price_usd": "0"}]}),
            json!({"offers": [{"price_usd": "1"}]}),
            json!({"offers": []}),
        ] {
            assert!(Catalog::from_json(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn statuses_are_named_as_acp_names_them() {
        assert_eq!(
            Status::AuthenticationRequired.name(),
            "authentication_required"
        );
        assert_eq!(Status::CompleteInProgress.name(), "complete_in_progress");
        assert_eq!(
            serde_json::to_value(Status::ReadyForPayment).unwrap(),
            json!("ready_for_payment")
        );
    }
}
