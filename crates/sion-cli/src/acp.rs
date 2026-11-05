//! ACP, the Agentic Commerce Protocol (API version 2026-04-17): selling an
//! agent's work to other agents through a checkout they already speak,
//! paid in stablecoin USD on Siontology.
//!
//! [ACP](https://github.com/agentic-commerce-protocol/agentic-commerce-protocol)
//! is the REST contract a merchant serves so that an agent can buy from it:
//! `POST /checkout_sessions` makes a session from items, `POST
//! /checkout_sessions/{id}` changes it, `GET` reads it, `…/complete` pays
//! for it and makes the order, `…/cancel` ends it, and
//! `/.well-known/acp.json` says all this is served. The merchant is the
//! system of record: every answer is the whole session, authoritative.
//! This [`Merchant`] serves that contract as ACP has it: the `API-Version`
//! header, a bearer token, an `Idempotency-Key` on every `POST` (the same
//! key and body is answered with the stored answer, the same key with
//! another body is refused, 5xx answers are not kept, keys are kept 24
//! hours), integer minor units, and errors in ACP's flat shape.
//!
//! **Payment.** ACP's payment handlers are open by design: a handler is a
//! named specification (`dev.acp.tokenized.card` for cards through a PSP)
//! that the session declares, and an instrument that names it. This
//! merchant declares one, [`HANDLER`] (`org.siontology.ledger`): its
//! `config.requirements` is an x402 quote ([`crate::x402`]) for the
//! session's total, and the instrument's credential is a Siontology
//! transaction signed against it and not submitted. Completing the session
//! settles that transaction with the x402 [`Facilitator`]: submitted,
//! executed, its receipt verified as a light client does against the
//! pinned chain, and only then is the order made. It needs no delegated
//! card token (`requires_delegate_payment: false`): what bounds the agent
//! is its Agent Charter on the ledger, which caps whom it pays and how
//! much. Card handlers are not served here, so an agent that knows only
//! those cannot pay this merchant.
//!
//! **AP2.** A merchant made [`Merchant::with_ap2`] also takes the Agent
//! Payments Protocol's checkout mandate ([`crate::ap2`]), through an ACP
//! extension, [`AP2_EXTENSION`]: each session carries a checkout JWT this
//! merchant signed (UCP's shape: id, merchant, line items, totals), with
//! an audience and a nonce; `…/complete` may carry `ap2.checkout_mandate`
//! (and `ap2.payment_mandate`), which is checked before any money moves;
//! the session then carries the signed checkout receipt (and the payment
//! receipt). A merchant may require the mandate.
//!
//! What is not served: ACP's `Signature` and `Timestamp` request signing
//! (a bearer token is checked, no signature), webhooks, the delegate
//! payment and delegate authentication APIs, 3-D Secure, the feed and cart
//! APIs, discounts and fulfillment beyond digital. Sessions are kept in
//! memory and go when the merchant stops; orders are written down.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use sion_core::ids::AccountId;
use sion_core::Amount;

use crate::ap2::{self, Refusal, SigningKey, Trust};
use crate::error::CliError;
use crate::x402::{Facilitator, PaymentPayload, PaymentRequirements, ResourceInfo, X402_VERSION};

/// The ACP version served.
pub const API_VERSION: &str = "2026-04-17";

/// The payment handler this merchant declares: settled on Siontology.
pub const HANDLER: &str = "org.siontology.ledger";
/// The version of [`HANDLER`]'s specification.
pub const HANDLER_VERSION: &str = "2026-11-05";
/// The handler's id within a session.
pub const HANDLER_ID: &str = "sion_ledger";
/// The instrument type of [`HANDLER`].
pub const INSTRUMENT: &str = "sion_transaction";
/// Where [`HANDLER`] is specified.
pub const HANDLER_SPEC: &str = "https://siontology.org/docs/agent/commerce.html";

/// The ACP extension that carries AP2's mandates and receipts.
pub const AP2_EXTENSION: &str = "org.siontology.ap2";

/// Micro-USD in a cent: ACP's minor unit for USD is the cent.
pub const MICROS_PER_CENT: u128 = 10_000;

/// How long a session's quote holds.
pub const QUOTE_FOR: Duration = Duration::from_secs(30 * 60);

/// How long an idempotency key's answer is kept.
pub const KEYS_KEPT: Duration = Duration::from_secs(24 * 3600);

/// The most sessions kept at once; the oldest finished ones go first.
const MAX_SESSIONS: usize = 10_000;

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

// ----- Requests and answers -----------------------------------------------

/// An HTTP request to the merchant.
#[derive(Clone, Debug)]
pub struct Request<'a> {
    /// `GET` or `POST`.
    pub method: &'a str,
    /// The path, without the query.
    pub path: &'a str,
    /// Its headers.
    pub headers: &'a [(String, String)],
    /// Its body.
    pub body: &'a str,
}

impl Request<'_> {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// The merchant's answer.
#[derive(Clone, Debug, PartialEq)]
pub struct Response {
    /// Its status.
    pub status: u16,
    /// Headers beyond `Content-Type: application/json`.
    pub headers: Vec<(String, String)>,
    /// Its body.
    pub body: Value,
}

/// An error in ACP's flat shape.
fn error(status: u16, kind: &str, code: &str, message: &str, param: Option<&str>) -> Response {
    let mut body = json!({"type": kind, "code": code, "message": message});
    if let Some(param) = param {
        body["param"] = json!(param);
    }
    Response {
        status,
        headers: Vec::new(),
        body,
    }
}

fn invalid(code: &str, message: &str, param: Option<&str>) -> Response {
    error(400, "invalid_request", code, message, param)
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// RFC 3339 of seconds since 1970, in UTC.
pub fn rfc3339(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let rest = seconds % 86_400;
    // Civil date from days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// A body as idempotency compares it: numbers that are whole are whole
/// (`1.0` is `1`); key order never mattered to a JSON value.
fn normalized(value: &Value) -> Value {
    match value {
        Value::Number(n) => match n.as_f64() {
            Some(f) if n.is_f64() && f.fract() == 0.0 && f.abs() < 9.0e15 => json!(f as i64),
            _ => value.clone(),
        },
        Value::Array(items) => Value::Array(items.iter().map(normalized).collect()),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(k, v)| (k.clone(), normalized(v)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn same_token(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

fn random_hex(bytes: usize) -> String {
    use aws_lc_rs::rand::SecureRandom;
    let mut buffer = vec![0u8; bytes];
    aws_lc_rs::rand::SystemRandom::new()
        .fill(&mut buffer)
        .expect("the system has randomness");
    hex::encode(buffer)
}

// ----- Sessions -----------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status {
    Ready,
    Completed,
    Canceled,
}

impl Status {
    fn name(self) -> &'static str {
        match self {
            Status::Ready => "ready_for_payment",
            Status::Completed => "completed",
            Status::Canceled => "canceled",
        }
    }
}

#[derive(Clone, Debug)]
struct Session {
    id: String,
    status: Status,
    lines: Vec<(Offer, u64)>,
    buyer: Option<Value>,
    requirements: PaymentRequirements,
    created: u64,
    updated: u64,
    expires: u64,
    /// The AP2 checkout this merchant signed for it, and the challenge.
    checkout_jwt: Option<String>,
    nonce: String,
    receipts: BTreeMap<&'static str, String>,
    messages: Vec<Value>,
    order: Option<Value>,
}

impl Session {
    fn total_cents(&self) -> u64 {
        self.lines
            .iter()
            .map(|(offer, quantity)| (offer.price.0 / MICROS_PER_CENT) as u64 * quantity)
            .sum()
    }
}

/// The mandates a completion came with, approved: the checkout's, and
/// the payment's if one came.
type Approved = (ap2::CheckoutApproved, Option<ap2::PaymentApproved>);

/// AP2 as a merchant takes it.
struct Ap2 {
    key: SigningKey,
    trust: Trust,
    required: bool,
    /// What was paid under each open payment mandate, in cents.
    spent: Mutex<BTreeMap<String, u64>>,
    /// The open checkout mandates an order was made under: AP2's line
    /// items cannot be split across checkouts, so each is used once.
    used: Mutex<std::collections::BTreeSet<String>>,
}

/// A stored answer to an idempotency key, or one still being made.
#[derive(Clone, Debug)]
enum Kept {
    InFlight(Value),
    Done(Value, Response, Instant),
}

/// A Siontology merchant that speaks ACP.
pub struct Merchant<'l> {
    facilitator: Facilitator<'l>,
    pay_to: AccountId,
    name: String,
    base_url: String,
    catalog: Catalog,
    tokens: Vec<String>,
    ap2: Option<Ap2>,
    sessions: Mutex<BTreeMap<String, Session>>,
    keys: Mutex<BTreeMap<(String, String, String), Kept>>,
    counter: AtomicU64,
    orders: Option<PathBuf>,
}

impl std::fmt::Debug for Merchant<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Merchant({}, {})", self.name, self.facilitator.network())
    }
}

impl<'l> Merchant<'l> {
    /// A merchant selling `catalog`, paid to `pay_to` and settled by
    /// `facilitator`, served at `base_url` to holders of `tokens` (each at
    /// least 16 characters).
    pub fn new(
        facilitator: Facilitator<'l>,
        pay_to: AccountId,
        name: &str,
        base_url: &str,
        catalog: Catalog,
        tokens: Vec<String>,
    ) -> Result<Self, CliError> {
        if tokens.is_empty() || tokens.iter().any(|t| t.len() < 16) {
            return Err(CliError::Usage(
                "a merchant's tokens have at least 16 characters, and there is one".into(),
            ));
        }
        Ok(Merchant {
            facilitator,
            pay_to,
            name: name.to_string(),
            base_url: base_url.trim_end_matches('/').to_string(),
            catalog,
            tokens,
            ap2: None,
            sessions: Mutex::new(BTreeMap::new()),
            keys: Mutex::new(BTreeMap::new()),
            counter: AtomicU64::new(0),
            orders: None,
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
            spent: Mutex::new(BTreeMap::new()),
            used: Mutex::new(std::collections::BTreeSet::new()),
        });
        self
    }

    /// Writes each order to `path`, one JSON line each.
    pub fn with_orders(mut self, path: PathBuf) -> Self {
        self.orders = Some(path);
        self
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
            "services": ["checkout"],
            "supported_currencies": ["usd"],
        });
        if self.ap2.is_some() {
            capabilities["extensions"] = json!([AP2_EXTENSION]);
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

    fn next_id(&self, prefix: &str) -> String {
        let count = self.counter.fetch_add(1, Ordering::Relaxed);
        format!("{prefix}_{count}_{}", random_hex(6))
    }

    /// Answers one request.
    pub fn answer(&self, request: &Request<'_>) -> Response {
        let mut response = self.answer_inner(request);
        if let Some(id) = request.header("Request-Id") {
            response.headers.push(("Request-Id".into(), id.to_string()));
        }
        if request.method == "POST" {
            if let Some(key) = request.header("Idempotency-Key") {
                response
                    .headers
                    .push(("Idempotency-Key".into(), key.to_string()));
            }
        }
        response
    }

    fn answer_inner(&self, request: &Request<'_>) -> Response {
        if request.method == "GET" {
            if request.path == "/.well-known/acp.json" {
                return Response {
                    status: 200,
                    headers: Vec::new(),
                    body: self.discovery(),
                };
            }
            // An order's page, and its payment's receipt: public, as a
            // buyer's link is.
            if let Some(id) = request.path.strip_prefix("/orders/") {
                return self.order_page(id);
            }
        }
        let presented = request
            .header("Authorization")
            .and_then(|v| v.strip_prefix("Bearer "));
        if !presented.is_some_and(|p| self.tokens.iter().any(|t| same_token(t, p))) {
            return error(
                401,
                "invalid_request",
                "unauthorized",
                "a bearer token this merchant issued is needed",
                None,
            );
        }
        match request.header("API-Version") {
            None => {
                let mut refused = invalid("missing_api_version", "API-Version is required", None);
                refused.body["supported_versions"] = json!([API_VERSION]);
                return refused;
            }
            Some(version) if version != API_VERSION => {
                let mut refused = invalid(
                    "unsupported_api_version",
                    &format!("API-Version {version} is not served"),
                    None,
                );
                refused.body["supported_versions"] = json!([API_VERSION]);
                return refused;
            }
            Some(_) => {}
        }
        let route = route_of(request.method, request.path);
        if route == Route::NotFound {
            return error(
                404,
                "invalid_request",
                "not_found",
                "no such endpoint",
                None,
            );
        }
        if request.method != "POST" {
            return self.route(route, &Value::Null);
        }
        let Some(key) = request.header("Idempotency-Key") else {
            return invalid(
                "idempotency_key_required",
                "Idempotency-Key header is required on all POST requests",
                None,
            );
        };
        if key.is_empty() || key.len() > 255 {
            return invalid(
                "invalid",
                "an Idempotency-Key has 1 to 255 characters",
                None,
            );
        }
        let body: Value = if request.body.trim().is_empty() {
            json!({})
        } else {
            match serde_json::from_str(request.body) {
                Ok(body) => body,
                Err(e) => return invalid("invalid", &format!("the body is not JSON: {e}"), None),
            }
        };
        if !body.is_object() {
            return invalid("invalid", "the body is a JSON object", None);
        }
        let body = normalized(&body);
        let slot = (
            ap2::hash(presented.unwrap_or_default()),
            request.path.to_string(),
            key.to_string(),
        );
        {
            let mut keys = self.keys.lock().expect("keys lock");
            keys.retain(|_, kept| match kept {
                Kept::Done(_, _, at) => at.elapsed() < KEYS_KEPT,
                Kept::InFlight(_) => true,
            });
            match keys.get(&slot) {
                Some(Kept::Done(stored, response, _)) if *stored == body => {
                    let mut replayed = response.clone();
                    replayed
                        .headers
                        .push(("Idempotent-Replayed".into(), "true".into()));
                    return replayed;
                }
                Some(Kept::InFlight(stored)) if *stored == body => {
                    let mut busy = error(
                        409,
                        "invalid_request",
                        "idempotency_in_flight",
                        "A request with this Idempotency-Key is currently being processed",
                        None,
                    );
                    busy.headers.push(("Retry-After".into(), "1".into()));
                    return busy;
                }
                Some(_) => {
                    return error(
                        422,
                        "invalid_request",
                        "idempotency_conflict",
                        "Idempotency-Key has already been used with a different request body",
                        None,
                    )
                }
                None => {
                    keys.insert(slot.clone(), Kept::InFlight(body.clone()));
                }
            }
        }
        let response = self.route(route, &body);
        let mut keys = self.keys.lock().expect("keys lock");
        if response.status >= 500 {
            keys.remove(&slot);
        } else {
            keys.insert(slot, Kept::Done(body, response.clone(), Instant::now()));
        }
        response
    }

    fn route(&self, route: Route, body: &Value) -> Response {
        match route {
            Route::Create => self.create(body),
            Route::Update(id) => self.update(&id, body),
            Route::Get(id) => self.with_session(&id, |merchant, session| Response {
                status: 200,
                headers: Vec::new(),
                body: merchant.view(session),
            }),
            Route::Complete(id) => self.complete(&id, body),
            Route::Cancel(id) => self.cancel(&id),
            Route::NotFound => error(
                404,
                "invalid_request",
                "not_found",
                "no such endpoint",
                None,
            ),
        }
    }

    fn lines_of(&self, items: &Value) -> Result<Vec<(Offer, u64)>, Response> {
        let items = items.as_array().filter(|i| !i.is_empty()).ok_or_else(|| {
            invalid(
                "missing",
                "line_items lists what is bought",
                Some("$.line_items"),
            )
        })?;
        let mut lines: Vec<(Offer, u64)> = Vec::new();
        for (index, item) in items.iter().enumerate() {
            let id = item["id"].as_str().unwrap_or_default();
            let Some(offer) = self.catalog.offers.get(id) else {
                return Err(invalid(
                    "invalid_item_id",
                    &format!("The item ID '{id}' does not exist."),
                    Some(&format!("$.line_items[{index}].id")),
                ));
            };
            // ACP's Item has no quantity: an item is named once for each
            // one bought. A `quantity` is taken too.
            let quantity = match item.get("quantity") {
                None => 1,
                Some(q) => match q.as_u64() {
                    Some(q) if (1..=10_000).contains(&q) => q,
                    _ => {
                        return Err(invalid(
                            "invalid",
                            "a quantity is 1 to 10000",
                            Some(&format!("$.line_items[{index}].quantity")),
                        ))
                    }
                },
            };
            match lines.iter_mut().find(|(o, _)| o.id == offer.id) {
                Some((_, q)) => *q += quantity,
                None => lines.push((offer.clone(), quantity)),
            }
        }
        Ok(lines)
    }

    /// Prices `session` afresh: a new quote, and a new AP2 checkout.
    fn quote(&self, session: &mut Session) {
        let total = Amount(u128::from(session.total_cents()) * MICROS_PER_CENT);
        let resource = ResourceInfo {
            url: format!("/checkout_sessions/{}", session.id),
            description: Some(format!("checkout session {}", session.id)),
            mime_type: Some("application/json".into()),
        };
        let quoted = self
            .facilitator
            .quote(resource, self.pay_to, total, QUOTE_FOR);
        session.requirements = quoted.accepts[0].clone();
        let now = now_seconds();
        session.updated = now;
        session.expires = now + QUOTE_FOR.as_secs();
        session.nonce = random_hex(16);
        if let Some(ap2) = &self.ap2 {
            session.checkout_jwt = Some(ap2::sign_jws(
                &json!({"typ": "JWT", "kid": "merchant"}),
                &self.checkout_of(session),
                &ap2.key,
            ));
        }
    }

    /// The checkout an AP2 mandate approves, in UCP's shape.
    fn checkout_of(&self, session: &Session) -> Value {
        let lines: Vec<Value> = session
            .lines
            .iter()
            .enumerate()
            .map(|(index, (offer, quantity))| {
                let price = (offer.price.0 / MICROS_PER_CENT) as u64;
                json!({
                    "id": format!("li_{index}"),
                    "item": {"id": offer.id, "title": offer.name, "price": price},
                    "quantity": quantity,
                    "totals": [
                        {"type": "subtotal", "amount": price * quantity},
                        {"type": "total", "amount": price * quantity},
                    ],
                })
            })
            .collect();
        let total = session.total_cents();
        json!({
            "id": session.id,
            "merchant": self.identity(),
            "line_items": lines,
            "status": "ready_for_complete",
            "currency": "USD",
            "totals": [
                {"type": "subtotal", "amount": total},
                {"type": "total", "amount": total},
            ],
            "iat": session.updated,
            "nonce": session.nonce,
        })
    }

    fn create(&self, body: &Value) -> Response {
        let currency = body["currency"].as_str().unwrap_or_default();
        if !currency.eq_ignore_ascii_case("usd") {
            return invalid("invalid", "this merchant sells in usd", Some("$.currency"));
        }
        let lines = match self.lines_of(&body["line_items"]) {
            Ok(lines) => lines,
            Err(refused) => return refused,
        };
        let now = now_seconds();
        let mut session = Session {
            id: self.next_id("cs"),
            status: Status::Ready,
            lines,
            buyer: body.get("buyer").cloned(),
            requirements: PaymentRequirements {
                scheme: String::new(),
                network: String::new(),
                asset: String::new(),
                amount: String::new(),
                pay_to: String::new(),
                max_timeout_seconds: 0,
                extra: Value::Null,
            },
            created: now,
            updated: now,
            expires: now,
            checkout_jwt: None,
            nonce: String::new(),
            receipts: BTreeMap::new(),
            messages: Vec::new(),
            order: None,
        };
        self.quote(&mut session);
        let view = self.view(&session);
        let mut sessions = self.sessions.lock().expect("sessions lock");
        if sessions.len() >= MAX_SESSIONS {
            let oldest = sessions
                .values()
                .filter(|s| s.status != Status::Ready || s.expires < now)
                .min_by_key(|s| s.updated)
                .or_else(|| sessions.values().min_by_key(|s| s.updated))
                .map(|s| s.id.clone());
            if let Some(oldest) = oldest {
                sessions.remove(&oldest);
            }
        }
        sessions.insert(session.id.clone(), session);
        Response {
            status: 201,
            headers: Vec::new(),
            body: view,
        }
    }

    fn with_session(
        &self,
        id: &str,
        then: impl FnOnce(&Self, &mut Session) -> Response,
    ) -> Response {
        let mut sessions = self.sessions.lock().expect("sessions lock");
        match sessions.get_mut(id) {
            Some(session) => then(self, session),
            None => error(
                404,
                "invalid_request",
                "not_found",
                "no such checkout session",
                None,
            ),
        }
    }

    fn update(&self, id: &str, body: &Value) -> Response {
        self.with_session(id, |merchant, session| {
            if session.status != Status::Ready {
                return invalid(
                    "invalid",
                    &format!("the session is {}", session.status.name()),
                    None,
                );
            }
            if let Some(items) = body.get("line_items") {
                match merchant.lines_of(items) {
                    Ok(lines) => session.lines = lines,
                    Err(refused) => return refused,
                }
            }
            if let Some(buyer) = body.get("buyer") {
                session.buyer = Some(buyer.clone());
            }
            session.messages.clear();
            merchant.quote(session);
            Response {
                status: 200,
                headers: Vec::new(),
                body: merchant.view(session),
            }
        })
    }

    fn cancel(&self, id: &str) -> Response {
        self.with_session(id, |merchant, session| {
            if session.status != Status::Ready {
                return error(
                    405,
                    "invalid_request",
                    "invalid",
                    &format!("the session is already {}", session.status.name()),
                    None,
                );
            }
            session.status = Status::Canceled;
            session.updated = now_seconds();
            Response {
                status: 200,
                headers: Vec::new(),
                body: merchant.view(session),
            }
        })
    }

    /// Checks the AP2 mandates `presented`, if any: the checkout's receipt
    /// and, if a payment mandate came, the payment's mandate.
    fn check_mandates(
        &self,
        session: &Session,
        presented: Option<&Value>,
    ) -> Result<Option<Approved>, (String, Refusal)> {
        let Some(ap2) = &self.ap2 else {
            return Ok(None);
        };
        let checkout_jwt = session.checkout_jwt.as_deref().expect("signed with ap2");
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
        let expect = ap2::Expect {
            audience: &self.pay_to.to_string(),
            nonce: &session.nonce,
            now: now_seconds(),
        };
        let approved =
            ap2::verify_checkout(chain, &ap2.trust, expect, checkout_jwt, &ap2.key.public())
                .map_err(|refusal| ("checkout".to_string(), refusal))?;
        if approved
            .open
            .iter()
            .any(|open| ap2.used.lock().expect("used lock").contains(open))
        {
            return Err((
                "checkout".into(),
                Refusal::InvalidMandate("an order was already made under this open mandate".into()),
            ));
        }
        let paid = match presented.and_then(|p| p["payment_mandate"].as_str()) {
            None => None,
            Some(chain) => {
                let identity = self.identity();
                let spent = ap2.spent.lock().expect("spent lock").clone();
                // What was spent under the open payment mandate, if one: it
                // is known only once the chain is read, so read it twice.
                let first = ap2::verify_chain(chain, &ap2.trust, expect)
                    .map_err(|refusal| ("payment".to_string(), refusal))?;
                let open = first.open().last().map(|(_, hash)| hash.to_string());
                let payment = ap2::Payment {
                    checkout_hash: &approved.checkout_hash,
                    payee: &identity,
                    amount: session.total_cents(),
                    currency: "USD",
                    checkout_open: &approved.open,
                    spent_before: Some(open.and_then(|o| spent.get(&o).copied()).unwrap_or(0)),
                };
                Some(
                    ap2::verify_payment(chain, &ap2.trust, expect, &payment)
                        .map_err(|refusal| ("payment".to_string(), refusal))?,
                )
            }
        };
        Ok(Some((approved, paid)))
    }

    fn complete(&self, id: &str, body: &Value) -> Response {
        // The session is held for the whole of it: a second completion
        // waits, and then finds it completed.
        self.with_session(id, |merchant, session| {
            if session.status != Status::Ready {
                return invalid(
                    "invalid",
                    &format!("the session is {}", session.status.name()),
                    None,
                );
            }
            let payment = &body["payment_data"];
            if payment.is_null() {
                return invalid("missing", "payment_data is required", Some("$.payment_data"));
            }
            if payment["handler_id"] != HANDLER_ID {
                return invalid(
                    "invalid",
                    &format!("the only handler here is {HANDLER_ID} ({HANDLER})"),
                    Some("$.payment_data.handler_id"),
                );
            }
            let instrument = &payment["instrument"];
            let transaction = instrument["credential"]["transaction"].as_str();
            let Some(transaction) = transaction.filter(|_| instrument["type"] == INSTRUMENT) else {
                return invalid(
                    "invalid",
                    &format!(
                        "the instrument is a {INSTRUMENT} whose credential carries the signed transaction"
                    ),
                    Some("$.payment_data.instrument"),
                );
            };
            if let Some(buyer) = body.get("buyer") {
                session.buyer = Some(buyer.clone());
            }
            session.messages.clear();
            // Mandates before money: nothing moves for a refused one.
            let mandates = match merchant.check_mandates(session, body.get("ap2")) {
                Ok(mandates) => mandates,
                Err((which, refusal)) => {
                    if let (Some(ap2), Some(chain)) = (&merchant.ap2, body["ap2"][format!("{which}_mandate")].as_str()) {
                        let reference = ap2::split(chain)
                            .ok()
                            .and_then(|tokens| tokens.last().and_then(|t| t.sd_hash().ok()))
                            .unwrap_or_default();
                        let kind = if which == "checkout" { ap2::Kind::Checkout } else { ap2::Kind::Payment };
                        session.receipts.insert(
                            if which == "checkout" { "checkout_receipt" } else { "payment_receipt" },
                            ap2::receipt(kind, &ap2.key, &merchant.base_url, &reference, Err(&refusal), now_seconds()),
                        );
                    }
                    session.messages.push(json!({
                        "type": "error",
                        "code": "invalid",
                        "param": format!("$.ap2.{which}_mandate"),
                        "content_type": "plain",
                        "content": format!("AP2 {which} mandate refused: {refusal}"),
                    }));
                    return Response { status: 200, headers: Vec::new(), body: merchant.view(session) };
                }
            };
            let paid = PaymentPayload {
                x402_version: X402_VERSION,
                resource: Some(ResourceInfo {
                    url: format!("/checkout_sessions/{}", session.id),
                    description: None,
                    mime_type: None,
                }),
                accepted: session.requirements.clone(),
                payload: json!({"transaction": transaction}),
                extensions: None,
            };
            let settled = merchant
                .facilitator
                .settle(&format!("/checkout_sessions/{}", session.id), &paid);
            if !settled.success {
                session.messages.push(json!({
                    "type": "error",
                    "code": "payment_declined",
                    "resolution": "recoverable",
                    "param": "$.payment_data",
                    "content_type": "plain",
                    "content": format!(
                        "the payment did not settle: {}",
                        settled.error_reason.unwrap_or_default()
                    ),
                }));
                // A failed payment needs a new quote to pay against.
                merchant.quote(session);
                return Response { status: 200, headers: Vec::new(), body: merchant.view(session) };
            }
            let order_id = merchant.next_id("ord");
            let pulse = settled.extensions.as_ref().map(|e| e["sion"]["pulse"].clone()).unwrap_or(Value::Null);
            let order = json!({
                "type": "order",
                "id": order_id,
                "checkout_session_id": session.id,
                "permalink_url": format!("{}/orders/{order_id}", merchant.base_url),
                "status": "confirmed",
                "totals": [{"type": "total", "display_text": "Total", "amount": session.total_cents()}],
            });
            session.status = Status::Completed;
            session.updated = now_seconds();
            session.order = Some(order.clone());
            if let (Some(ap2), Some((checkout, payment))) = (&merchant.ap2, &mandates) {
                ap2.used.lock().expect("used lock").extend(checkout.open.iter().cloned());
                session.receipts.insert(
                    "checkout_receipt",
                    ap2::receipt(ap2::Kind::Checkout, &ap2.key, &merchant.base_url, &checkout.reference, Ok(&json!({"order_id": order_id})), now_seconds()),
                );
                if let Some(payment) = payment {
                    if let Some(open) = &payment.open {
                        *ap2.spent.lock().expect("spent lock").entry(open.clone()).or_default() += session.total_cents();
                    }
                    session.receipts.insert(
                        "payment_receipt",
                        ap2::receipt(ap2::Kind::Payment, &ap2.key, &merchant.base_url, &payment.reference, Ok(&json!({
                            "payment_id": settled.transaction,
                            "psp_confirmation_id": settled.transaction,
                            "network_confirmation_id": format!("{}@{}", settled.network, pulse),
                        })), now_seconds()),
                    );
                }
            }
            let record = json!({
                "order": order,
                "lines": session.lines.iter().map(|(o, q)| json!({"id": o.id, "quantity": q})).collect::<Vec<_>>(),
                "total_cents": session.total_cents(),
                "buyer": session.buyer,
                "payer": settled.payer,
                "transaction": settled.transaction,
                "network": settled.network,
                "pulse": pulse,
                "ap2": mandates.as_ref().map(|(c, p)| json!({
                    "checkout_mandate": c.reference,
                    "payment_mandate": p.as_ref().map(|p| p.reference.clone()),
                })),
                "at": rfc3339(now_seconds()),
            });
            if let Some(path) = &merchant.orders {
                let written = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                    .and_then(|mut file| writeln!(file, "{record}"));
                if let Err(e) = written {
                    eprintln!("warning: order {order_id} was paid but not written to {}: {e}", path.display());
                }
            }
            Response { status: 200, headers: Vec::new(), body: merchant.view(session) }
        })
    }

    fn order_page(&self, id: &str) -> Response {
        let sessions = self.sessions.lock().expect("sessions lock");
        let found = sessions
            .values()
            .find(|s| s.order.as_ref().is_some_and(|o| o["id"] == id));
        match found {
            Some(session) => Response {
                status: 200,
                headers: Vec::new(),
                body: json!({
                    "order": session.order,
                    "checkout_session_id": session.id,
                    "merchant": self.identity(),
                    "receipts": session.receipts,
                }),
            },
            None => error(
                404,
                "invalid_request",
                "not_found",
                "no such order here",
                None,
            ),
        }
    }

    fn handler(&self, session: &Session) -> Value {
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
                "network": session.requirements.network,
                "chain": session.requirements.extra["chain"],
                "asset": "USD",
                "micros_per_cent": MICROS_PER_CENT as u64,
                "pay_to": self.pay_to.to_string(),
                "requirements": session.requirements,
            },
            "display_order": 0,
        })
    }

    /// The session as ACP answers it.
    fn view(&self, session: &Session) -> Value {
        let lines: Vec<Value> = session
            .lines
            .iter()
            .enumerate()
            .map(|(index, (offer, quantity))| {
                let unit = (offer.price.0 / MICROS_PER_CENT) as u64;
                let amount = unit * quantity;
                json!({
                    "id": format!("li_{index}"),
                    "item": {"id": offer.id},
                    "quantity": quantity,
                    "name": offer.name,
                    "description": offer.description,
                    "unit_amount": unit,
                    "totals": [
                        {"type": "items_base_amount", "display_text": "Base Amount", "amount": amount},
                        {"type": "subtotal", "display_text": "Subtotal", "amount": amount},
                        {"type": "total", "display_text": "Total", "amount": amount},
                    ],
                })
            })
            .collect();
        let total = session.total_cents();
        let item_ids: Vec<&str> = session.lines.iter().map(|(o, _)| o.id.as_str()).collect();
        let mut capabilities = json!({
            "payment": {"handlers": if session.status == Status::Ready { vec![self.handler(session)] } else { Vec::new() }},
        });
        if self.ap2.is_some() {
            capabilities["extensions"] = json!([{
                "name": AP2_EXTENSION,
                "extends": ["$.CheckoutSession.ap2", "$.CheckoutSessionCompleteRequest.ap2"],
                "schema": "https://siontology.org/docs/agent/commerce/ap2.schema.json",
            }]);
        }
        let mut view = json!({
            "id": session.id,
            "protocol": {"version": API_VERSION},
            "capabilities": capabilities,
            "status": session.status.name(),
            "currency": "usd",
            "line_items": lines,
            "fulfillment_options": [{
                "type": "digital",
                "id": "agent_delivery",
                "title": "Delivered by the agent",
                "totals": [{"type": "fulfillment", "display_text": "Delivery", "amount": 0}],
            }],
            "selected_fulfillment_options": [{"type": "digital", "option_id": "agent_delivery", "item_ids": item_ids}],
            "totals": [
                {"type": "items_base_amount", "display_text": "Item(s) total", "amount": total},
                {"type": "subtotal", "display_text": "Subtotal", "amount": total},
                {"type": "total", "display_text": "Total", "amount": total},
            ],
            "messages": session.messages,
            "links": [],
            "created_at": rfc3339(session.created),
            "updated_at": rfc3339(session.updated),
            "expires_at": rfc3339(session.expires),
        });
        if let Some(buyer) = &session.buyer {
            view["buyer"] = buyer.clone();
        }
        if let Some(order) = &session.order {
            view["order"] = order.clone();
        }
        if self.ap2.is_some() {
            let mut ap2 = json!({
                "checkout_jwt": session.checkout_jwt,
                "audience": self.pay_to.to_string(),
                "nonce": session.nonce,
                "merchant": self.identity(),
            });
            for (name, receipt) in &session.receipts {
                ap2[*name] = json!(receipt);
            }
            view["ap2"] = ap2;
        }
        view
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Route {
    Create,
    Update(String),
    Get(String),
    Complete(String),
    Cancel(String),
    NotFound,
}

fn route_of(method: &str, path: &str) -> Route {
    let Some(rest) = path.strip_prefix("/checkout_sessions") else {
        return Route::NotFound;
    };
    let parts: Vec<&str> = rest.split('/').filter(|p| !p.is_empty()).collect();
    match (method, parts.as_slice()) {
        ("POST", []) => Route::Create,
        ("POST", [id]) => Route::Update(id.to_string()),
        ("GET", [id]) => Route::Get(id.to_string()),
        ("POST", [id, "complete"]) => Route::Complete(id.to_string()),
        ("POST", [id, "cancel"]) => Route::Cancel(id.to_string()),
        _ => Route::NotFound,
    }
}

// ----- Serving ------------------------------------------------------------

/// The longest request body taken, in bytes.
pub const MAX_BODY: u64 = 1 << 20;

/// Serves `merchant` on `server` until stopped (or for `requests`
/// requests), several at once: a completion waits for its payment's
/// receipt.
pub fn serve(merchant: &Merchant<'_>, server: &tiny_http::Server, requests: Option<usize>) {
    let at_once = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for (index, mut request) in server.incoming_requests().enumerate() {
            while at_once.load(Ordering::SeqCst) >= crate::channels::AT_ONCE {
                std::thread::park_timeout(Duration::from_millis(5));
            }
            at_once.fetch_add(1, Ordering::SeqCst);
            let at_once = &at_once;
            scope.spawn(move || {
                let headers: Vec<(String, String)> = request
                    .headers()
                    .iter()
                    .map(|h| (h.field.as_str().to_string(), h.value.as_str().to_string()))
                    .collect();
                let path = request
                    .url()
                    .split('?')
                    .next()
                    .unwrap_or_default()
                    .to_string();
                let method = request.method().as_str().to_string();
                let mut body = String::new();
                let read = request
                    .as_reader()
                    .take(MAX_BODY + 1)
                    .read_to_string(&mut body);
                let response = if read.is_err() || body.len() as u64 > MAX_BODY {
                    error(
                        413,
                        "invalid_request",
                        "invalid",
                        "the body is too long",
                        None,
                    )
                } else {
                    merchant.answer(&Request {
                        method: &method,
                        path: &path,
                        headers: &headers,
                        body: &body,
                    })
                };
                let mut reply = tiny_http::Response::from_string(response.body.to_string())
                    .with_status_code(response.status)
                    .with_header(
                        tiny_http::Header::from_bytes(
                            &b"Content-Type"[..],
                            &b"application/json"[..],
                        )
                        .expect("a valid header"),
                    );
                for (name, value) in &response.headers {
                    if let Ok(header) =
                        tiny_http::Header::from_bytes(name.as_bytes(), value.as_bytes())
                    {
                        reply = reply.with_header(header);
                    }
                }
                let _ = request.respond(reply);
                at_once.fetch_sub(1, Ordering::SeqCst);
            });
            if requests.is_some_and(|most| index + 1 >= most) {
                break;
            }
        }
    });
}

// ----- Buying -------------------------------------------------------------

/// A buyer's side of a checkout: what an agent on Siontology does to buy
/// from an ACP merchant that takes [`HANDLER`].
pub struct Buyer<'a> {
    /// The merchant's API base URL.
    pub base_url: &'a str,
    /// The bearer token the merchant issued.
    pub token: &'a str,
    /// The chain payments are checked against (pinned, not the merchant's
    /// word).
    pub chain: sion_core::hash::Hash32,
    /// The most the buyer pays, in micro-USD.
    pub max_price: Amount,
}

impl std::fmt::Debug for Buyer<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The token is left out: it is the merchant's secret too.
        write!(
            f,
            "Buyer({}, at most {} USD)",
            self.base_url, self.max_price
        )
    }
}

fn call(
    buyer: &Buyer<'_>,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Result<(u16, Value), CliError> {
    let url = format!("{}{path}", buyer.base_url.trim_end_matches('/'));
    let mut request = ureq::request(method, &url)
        .set("Authorization", &format!("Bearer {}", buyer.token))
        .set("API-Version", API_VERSION)
        .timeout(Duration::from_secs(120));
    let answer = match body {
        Some(body) => {
            request = request.set("Idempotency-Key", &random_hex(16));
            request.send_json(body.clone())
        }
        None => request.call(),
    };
    let response = match answer {
        Ok(response) => response,
        Err(ureq::Error::Status(_, response)) => response,
        Err(e) => return Err(CliError::Connection(e.to_string())),
    };
    let status = response.status();
    let text = response.into_string().unwrap_or_default();
    let value = serde_json::from_str(&text).unwrap_or(json!({"message": text}));
    Ok((status, value))
}

/// Buys `items` (offer ids, one per unit): makes a checkout session, checks
/// its total and its payment terms, pays as `payer`, and completes it with
/// what `mandates` gives for the session (its `ap2` field, if the merchant
/// takes AP2). The completed session, or why not.
pub fn buy(
    session: &mut crate::session::Session<'_>,
    payer: &crate::x402::Payer,
    buyer: &Buyer<'_>,
    items: &[&str],
    mandates: impl FnOnce(&Value) -> Result<Option<Value>, CliError>,
) -> Result<Value, CliError> {
    let line_items: Vec<Value> = items.iter().map(|id| json!({"id": id})).collect();
    let (status, created) = call(
        buyer,
        "POST",
        "/checkout_sessions",
        Some(&json!({"currency": "usd", "line_items": line_items, "capabilities": {}})),
    )?;
    if status != 201 {
        return Err(CliError::Protocol(format!(
            "the merchant did not make a session ({status}): {}",
            created["message"]
        )));
    }
    let id = created["id"].as_str().unwrap_or_default().to_string();
    let handler = created["capabilities"]["payment"]["handlers"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|h| h["name"] == HANDLER)
        .cloned()
        .ok_or_else(|| CliError::Protocol(format!("the merchant takes no {HANDLER} payment")))?;
    let requirements: PaymentRequirements =
        serde_json::from_value(handler["config"]["requirements"].clone())
            .map_err(|e| CliError::Protocol(format!("the handler's requirements: {e}")))?;
    if requirements.extra["chain"].as_str() != Some(buyer.chain.to_hex().as_str()) {
        return Err(CliError::Protocol(
            "the merchant is paid on another chain than the one pinned".into(),
        ));
    }
    let total_cents = created["totals"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|t| t["type"] == "total")
        .and_then(|t| t["amount"].as_u64())
        .unwrap_or(u64::MAX);
    let price: u128 = requirements.amount.parse().unwrap_or(u128::MAX);
    if price != u128::from(total_cents).saturating_mul(MICROS_PER_CENT) {
        return Err(CliError::Protocol(
            "the quote is not the session's total".into(),
        ));
    }
    if price > buyer.max_price.0 {
        let _ = call(
            buyer,
            "POST",
            &format!("/checkout_sessions/{id}/cancel"),
            Some(&json!({})),
        );
        return Err(CliError::Usage(format!(
            "the checkout costs {} USD, more than the {} USD allowed",
            Amount(price),
            buyer.max_price
        )));
    }
    let signed = crate::x402::sign_payment(session, payer, &requirements)?;
    let transaction = hex::encode(sion_core::encode::encode(&signed));
    let mut complete = json!({
        "payment_data": {
            "handler_id": handler["id"],
            "instrument": {
                "id": random_hex(8),
                "handler_id": handler["id"],
                "type": INSTRUMENT,
                "credential": {"type": "signed_transaction", "transaction": transaction},
            },
        },
    });
    match mandates(&created) {
        Ok(Some(ap2)) => complete["ap2"] = ap2,
        Ok(None) => {}
        Err(e) => {
            session.settle_all();
            return Err(e);
        }
    }
    let answered = call(
        buyer,
        "POST",
        &format!("/checkout_sessions/{id}/complete"),
        Some(&complete),
    );
    // Paid or not, the coins the payment was signed from are the ledger's
    // to say: a refused payment was never submitted.
    session.settle_all();
    let (status, done) = answered?;
    if status != 200 {
        return Err(CliError::Protocol(format!(
            "the merchant refused the completion ({status}): {}",
            done["message"]
        )));
    }
    if done["status"] != "completed" {
        let said: Vec<&str> = done["messages"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|m| m["content"].as_str())
            .collect();
        return Err(CliError::Protocol(format!(
            "the checkout did not complete: {}",
            said.join("; ")
        )));
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_route_to_acps_five_endpoints() {
        assert_eq!(route_of("POST", "/checkout_sessions"), Route::Create);
        assert_eq!(
            route_of("POST", "/checkout_sessions/cs_1"),
            Route::Update("cs_1".into())
        );
        assert_eq!(
            route_of("GET", "/checkout_sessions/cs_1"),
            Route::Get("cs_1".into())
        );
        assert_eq!(
            route_of("POST", "/checkout_sessions/cs_1/complete"),
            Route::Complete("cs_1".into())
        );
        assert_eq!(
            route_of("POST", "/checkout_sessions/cs_1/cancel"),
            Route::Cancel("cs_1".into())
        );
        assert_eq!(route_of("GET", "/checkout_sessions"), Route::NotFound);
        assert_eq!(
            route_of("DELETE", "/checkout_sessions/cs_1"),
            Route::NotFound
        );
        assert_eq!(route_of("POST", "/elsewhere"), Route::NotFound);
    }

    #[test]
    fn idempotency_compares_bodies_as_json_values() {
        let a: Value = serde_json::from_str(r#"{"b": 1.0, "a": [1, 2]}"#).unwrap();
        let b: Value = serde_json::from_str(r#"{"a": [1, 2], "b": 1}"#).unwrap();
        assert_eq!(normalized(&a), normalized(&b));
        // Order inside an array matters; null is not absent.
        let c: Value = serde_json::from_str(r#"{"a": [2, 1], "b": 1}"#).unwrap();
        assert_ne!(normalized(&a), normalized(&c));
        let d: Value = serde_json::from_str(r#"{"a": [1, 2], "b": 1, "c": null}"#).unwrap();
        assert_ne!(normalized(&b), normalized(&d));
    }

    #[test]
    fn times_are_written_as_rfc_3339() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(1_777_593_599), "2026-04-30T23:59:59Z");
        assert_eq!(
            ap2::seconds_of(&rfc3339(1_800_000_000)),
            Some(1_800_000_000)
        );
    }

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
}
