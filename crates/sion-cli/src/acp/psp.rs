//! A PSP for delegated cards: ACP's Delegate Payment API.
//!
//! An agent that holds a buyer's card does not give it to the merchant. It
//! gives it to the merchant's PSP, `POST /agentic_commerce/delegate_payment`,
//! with an *allowance*: one time, at most this much, in this currency, for
//! this checkout of this merchant, until then. The PSP keeps the card and
//! answers a vault token (`vt_…`), which the agent hands the merchant; the
//! merchant charges it, and the PSP charges the card only within the
//! allowance, once. That endpoint is served as ACP has it (API version
//! 2026-04-17): validation (§7), idempotency (§5: 422 on conflict, 409 in
//! flight, 5xx not kept, 24 hours), the signature over the canonical body
//! when the agent signs, and the flat errors. The discovery document names
//! the `delegate_payment` service.
//!
//! ACP leaves to the PSP how a merchant redeems a token; this PSP takes
//! `POST /agentic_commerce/create_and_process_payment_intent` with
//! `{vault_token, amount, currency, authentication_result?}`, the shape
//! NVIDIA's ACP reference implementation uses, from merchants it knows,
//! each only for tokens whose allowance names it.
//!
//! **Card data.** A card is sealed with AES-256-GCM under a key made when
//! the PSP starts and never written down; tokens therefore end when it
//! stops. Nothing here logs or prints a card number or CVC (their `Debug`
//! is redacted); the CVC is dropped once a charge is authorized. None of
//! this makes it PCI DSS compliant: running it with real cards needs that
//! assessed, and nothing here claims it.
//!
//! **Charging.** The card is charged by an [`Acquirer`]. The one here is
//! [`SimulatedAcquirer`]: a simulated card network that authorizes or
//! declines by test card number and moves no money. No acquirer that
//! reaches a card network is built.

use std::collections::BTreeMap;
use std::sync::Mutex;

use aws_lc_rs::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM};
use aws_lc_rs::rand::SecureRandom;
use serde_json::{json, Value};
use zeroize::Zeroizing;

use super::http::{self, Gate, Keys, Request, Response, Service};

/// The card number, expiry, name and CVC of a delegated card.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct CardData {
    /// The number (FPAN, DPAN or network token).
    pub number: String,
    /// Expiry month, `01`–`12`.
    pub exp_month: Option<String>,
    /// Expiry year, four digits.
    pub exp_year: Option<String>,
    /// The cardholder's name.
    pub name: Option<String>,
    /// The CVC.
    pub cvc: Option<String>,
}

impl std::fmt::Debug for CardData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the number or the CVC: its last four only.
        write!(f, "Card(…{})", last4(&self.number))
    }
}

impl Drop for CardData {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.number.zeroize();
        if let Some(cvc) = self.cvc.as_mut() {
            cvc.zeroize();
        }
    }
}

fn last4(number: &str) -> &str {
    &number[number.len().saturating_sub(4)..]
}

/// Whether `number` passes the Luhn check.
pub fn luhn(number: &str) -> bool {
    let digits: Vec<u32> = number.chars().filter_map(|c| c.to_digit(10)).collect();
    if digits.len() != number.len() || digits.len() < 12 {
        return false;
    }
    let sum: u32 = digits
        .iter()
        .rev()
        .enumerate()
        .map(|(i, &d)| {
            if i % 2 == 1 {
                let doubled = d * 2;
                if doubled > 9 {
                    doubled - 9
                } else {
                    doubled
                }
            } else {
                d
            }
        })
        .sum();
    sum % 10 == 0
}

/// The brand a card number belongs to, by its first digits.
pub fn brand_of(number: &str) -> &'static str {
    let first2: u32 = number.get(..2).and_then(|d| d.parse().ok()).unwrap_or(0);
    let first4: u32 = number.get(..4).and_then(|d| d.parse().ok()).unwrap_or(0);
    match number.as_bytes().first() {
        Some(b'4') => "visa",
        _ if (51..=55).contains(&first2) || (2221..=2720).contains(&first4) => "mastercard",
        _ if first2 == 34 || first2 == 37 => "amex",
        _ if number.starts_with("6011") || number.starts_with("65") => "discover",
        _ => "unknown",
    }
}

// ----- Acquiring ----------------------------------------------------------

/// Who charges a card the PSP holds.
pub trait Acquirer: Send + Sync {
    /// What it is, said wherever its answers are shown.
    fn label(&self) -> &'static str;

    /// Authorizes `amount` (minor units of `currency`) on `card` for
    /// `merchant`, with the 3-D Secure result if there is one: an
    /// authorization id, or why not (`authentication_required` when the
    /// issuer wants the buyer authenticated).
    fn authorize(
        &self,
        card: &CardData,
        amount: u64,
        currency: &str,
        merchant: &str,
        authentication: Option<&Value>,
    ) -> Result<String, String>;
}

/// A simulated card network: it authorizes or declines by test card number
/// and moves no money. For devnets and tests only; its answers say so.
///
/// | Card | Answer |
/// | --- | --- |
/// | `4000000000000002` | declined (`card_declined`) |
/// | `4000000000009995` | declined (`insufficient_funds`) |
/// | `4000000000003220`, `4000000000003063` | `authentication_required` unless the 3-D Secure outcome is `authenticated` |
/// | any other Luhn-valid number | authorized |
#[derive(Clone, Copy, Debug, Default)]
pub struct SimulatedAcquirer;

impl Acquirer for SimulatedAcquirer {
    fn label(&self) -> &'static str {
        "simulated card network (no money moves)"
    }

    fn authorize(
        &self,
        card: &CardData,
        amount: u64,
        _currency: &str,
        _merchant: &str,
        authentication: Option<&Value>,
    ) -> Result<String, String> {
        if amount == 0 {
            return Err("invalid_amount".into());
        }
        match card.number.as_str() {
            "4000000000000002" => Err("card_declined".into()),
            "4000000000009995" => Err("insufficient_funds".into()),
            "4000000000003220" | "4000000000003063" => {
                if authentication.is_some_and(|a| a["outcome"] == "authenticated") {
                    Ok(http::fresh_id("auth"))
                } else {
                    Err("authentication_required".into())
                }
            }
            number if luhn(number) => Ok(http::fresh_id("auth")),
            _ => Err("invalid_card".into()),
        }
    }
}

// ----- The vault ----------------------------------------------------------

/// A card sealed under the PSP's key.
struct Sealed {
    nonce: [u8; 12],
    ciphertext: Vec<u8>,
}

struct Vaulted {
    sealed: Sealed,
    allowance: Value,
    expires: u64,
    consumed: bool,
    brand: &'static str,
    last4: String,
}

#[derive(Default)]
struct Book {
    keys: Keys,
    vault: BTreeMap<String, Vaulted>,
    intents: BTreeMap<String, Value>,
}

/// A PSP serving ACP's delegate payment API.
pub struct Psp {
    agents: Gate,
    merchants: Gate,
    /// The merchant id each merchant client may redeem for.
    merchant_ids: BTreeMap<String, String>,
    acquirer: Box<dyn Acquirer>,
    key: LessSafeKey,
    book: Mutex<Book>,
    base_url: String,
}

impl std::fmt::Debug for Psp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Psp({}, {})", self.base_url, self.acquirer.label())
    }
}

impl Psp {
    /// A PSP at `base_url` taking cards from `agents` and redemptions from
    /// `merchants` (each with the merchant id it redeems for), charged by
    /// `acquirer`.
    pub fn new(
        base_url: &str,
        agents: Vec<http::Client>,
        merchants: Vec<(http::Client, String)>,
        acquirer: Box<dyn Acquirer>,
    ) -> Result<Psp, crate::error::CliError> {
        let mut key_bytes = Zeroizing::new([0u8; 32]);
        aws_lc_rs::rand::SystemRandom::new()
            .fill(key_bytes.as_mut())
            .expect("the system has randomness");
        let key = LessSafeKey::new(
            UnboundKey::new(&AES_256_GCM, key_bytes.as_ref()).expect("a 256-bit key"),
        );
        let merchant_ids = merchants
            .iter()
            .map(|(client, id)| (client.name.clone(), id.clone()))
            .collect();
        Ok(Psp {
            agents: Gate::new(agents, &[super::API_VERSION])?,
            merchants: Gate::new(
                merchants.into_iter().map(|(c, _)| c).collect(),
                &[super::API_VERSION],
            )?,
            merchant_ids,
            acquirer,
            key,
            book: Mutex::new(Book::default()),
            base_url: base_url.trim_end_matches('/').to_string(),
        })
    }

    /// What charges its cards.
    pub fn acquirer(&self) -> &dyn Acquirer {
        self.acquirer.as_ref()
    }

    fn seal(&self, card: &CardData) -> Sealed {
        let mut nonce = [0u8; 12];
        aws_lc_rs::rand::SystemRandom::new()
            .fill(&mut nonce)
            .expect("the system has randomness");
        let mut buffer = Zeroizing::new(serde_json::to_vec(card).expect("a card serializes"));
        let mut ciphertext = buffer.to_vec();
        self.key
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce),
                Aad::empty(),
                &mut ciphertext,
            )
            .expect("sealing succeeds");
        buffer.iter_mut().for_each(|b| *b = 0);
        Sealed { nonce, ciphertext }
    }

    fn open(&self, sealed: &Sealed) -> Option<CardData> {
        let mut buffer = Zeroizing::new(sealed.ciphertext.clone());
        let plain = self
            .key
            .open_in_place(
                Nonce::assume_unique_for_key(sealed.nonce),
                Aad::empty(),
                buffer.as_mut_slice(),
            )
            .ok()?;
        serde_json::from_slice(plain).ok()
    }

    /// The discovery document: the delegate payment service.
    pub fn discovery(&self) -> Value {
        json!({
            "protocol": {
                "name": "acp",
                "version": super::API_VERSION,
                "supported_versions": [super::API_VERSION],
            },
            "api_base_url": self.base_url,
            "transports": ["rest"],
            "capabilities": {"services": ["delegate_payment"], "supported_currencies": ["usd"]},
        })
    }

    fn delegate(&self, key: &str, body: &Value) -> Response {
        let (card, allowance) = match validate(body) {
            Ok(checked) => checked,
            Err(refused) => return refused,
        };
        let id = http::fresh_id("vt");
        let created = http::now();
        let expires = allowance["expires_at"]
            .as_str()
            .and_then(http::seconds_of)
            .unwrap_or(0);
        let mut metadata = body["metadata"].clone();
        metadata["merchant_id"] = allowance["merchant_id"].clone();
        metadata["idempotency_key"] = json!(key);
        let vaulted = Vaulted {
            sealed: self.seal(&card),
            allowance,
            expires,
            consumed: false,
            brand: brand_of(&card.number),
            last4: last4(&card.number).to_string(),
        };
        self.book
            .lock()
            .expect("psp lock")
            .vault
            .insert(id.clone(), vaulted);
        Response::json(
            201,
            json!({"id": id, "created": http::rfc3339(created), "metadata": metadata}),
        )
    }

    fn redeem(&self, merchant: &str, body: &Value) -> Response {
        let token = body["vault_token"].as_str().unwrap_or_default();
        let amount = body["amount"].as_u64().filter(|a| *a > 0);
        let currency = body["currency"]
            .as_str()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let Some(amount) = amount else {
            return http::error(
                422,
                "invalid_request",
                "invalid_amount",
                "amount is a positive integer of minor units",
                Some("$.amount"),
            );
        };
        let authentication = body.get("authentication_result").filter(|a| !a.is_null());
        if let Some(result) = authentication {
            if let Err(why) = super::card::check_authentication(result) {
                return http::invalid("invalid", &why, Some("$.authentication_result"));
            }
        }
        let merchant_id = self.merchant_ids.get(merchant).cloned().unwrap_or_default();
        let now = http::now();
        let mut book = self.book.lock().expect("psp lock");
        let Some(vaulted) = book.vault.get_mut(token) else {
            return http::error(
                404,
                "invalid_request",
                "vault_token_not_found",
                "no such vault token",
                Some("$.vault_token"),
            );
        };
        // A token is for the merchant its allowance names, and no other.
        if vaulted.allowance["merchant_id"] != merchant_id.as_str() {
            return http::error(
                404,
                "invalid_request",
                "vault_token_not_found",
                "no such vault token",
                Some("$.vault_token"),
            );
        }
        if vaulted.consumed {
            return http::error(
                409,
                "invalid_request",
                "vault_token_consumed",
                "the vault token was already used",
                Some("$.vault_token"),
            );
        }
        if now >= vaulted.expires {
            return http::error(
                410,
                "invalid_request",
                "vault_token_expired",
                "the vault token's allowance has expired",
                Some("$.vault_token"),
            );
        }
        let max = vaulted.allowance["max_amount"].as_u64().unwrap_or(0);
        if amount > max {
            return http::error(
                422,
                "invalid_request",
                "amount_exceeds_allowance",
                &format!("{amount} is more than the allowance's {max}"),
                Some("$.amount"),
            );
        }
        if vaulted.allowance["currency"] != currency.as_str() {
            return http::error(
                422,
                "invalid_request",
                "currency_mismatch",
                "the allowance is in another currency",
                Some("$.currency"),
            );
        }
        let Some(mut card) = self.open(&vaulted.sealed) else {
            return http::error(
                500,
                "processing_error",
                "vault_unreadable",
                "the card could not be read",
                None,
            );
        };
        match self
            .acquirer
            .authorize(&card, amount, &currency, &merchant_id, authentication)
        {
            Ok(authorization) => {
                vaulted.consumed = true;
                // The CVC is not kept past authorization.
                card.cvc = None;
                vaulted.sealed = self.seal(&card);
                let id = http::fresh_id("pi");
                let intent = json!({
                    "id": id,
                    "vault_token_id": token,
                    "amount": amount,
                    "currency": currency,
                    "status": "completed",
                    "created_at": http::rfc3339(now),
                    "completed_at": http::rfc3339(now),
                    "authorization": authorization,
                    "card": {"brand": vaulted.brand, "last4": vaulted.last4},
                    "acquirer": self.acquirer.label(),
                });
                book.intents.insert(id, intent.clone());
                Response::json(200, intent)
            }
            Err(why) if why == "authentication_required" => http::error(
                402,
                "invalid_request",
                "authentication_required",
                "the issuer asks for the buyer to be authenticated (3-D Secure)",
                None,
            ),
            Err(why) => http::error(
                402,
                "invalid_request",
                "payment_declined",
                &format!("the card was declined: {why}"),
                None,
            ),
        }
    }

    /// How many tokens it holds, and how many were used (for tests and the
    /// operator; never a card).
    pub fn holdings(&self) -> (usize, usize) {
        let book = self.book.lock().expect("psp lock");
        let used = book.vault.values().filter(|v| v.consumed).count();
        (book.vault.len(), used)
    }
}

impl Service for Psp {
    fn answer(&self, request: &Request<'_>) -> Response {
        let answer = match (request.method, request.path) {
            ("GET", "/.well-known/acp.json") => Response::json(200, self.discovery()),
            ("POST", "/agentic_commerce/delegate_payment") => {
                let client = match self.agents.admit(request) {
                    Ok(client) => client,
                    Err(refused) => return http::echo(request, refused),
                };
                self.keyed(request, &client.name, true, |body, key| {
                    self.delegate(key.unwrap_or_default(), body)
                })
            }
            ("POST", "/agentic_commerce/create_and_process_payment_intent") => {
                let client = match self.merchants.admit(request) {
                    Ok(client) => client,
                    Err(refused) => return http::echo(request, refused),
                };
                self.keyed(request, &client.name, false, |body, _| {
                    self.redeem(&client.name, body)
                })
            }
            _ => http::not_found("no such endpoint"),
        };
        http::echo(request, answer)
    }

    fn tick(&self) {
        // Expired tokens go, and their cards with them.
        let now = http::now();
        self.book
            .lock()
            .expect("psp lock")
            .vault
            .retain(|_, v| now < v.expires.saturating_add(3600));
    }
}

impl Psp {
    /// Runs `then` under the request's idempotency key (`required`, or
    /// optional).
    fn keyed(
        &self,
        request: &Request<'_>,
        client: &str,
        required: bool,
        then: impl FnOnce(&Value, Option<&str>) -> Response,
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
            return then(&body, None);
        };
        let slot = {
            let mut book = self.book.lock().expect("psp lock");
            match http::claim(&mut book.keys, 422, client, request.path, key, &body) {
                Ok(slot) => slot,
                Err(answered) => return answered,
            }
        };
        let response = then(&body, Some(key));
        http::settle(
            &mut self.book.lock().expect("psp lock").keys,
            slot,
            &response,
        );
        response
    }
}

/// Checks a delegate payment request against ACP's rules (§7, and the
/// schema's): the card and the allowance, or a refusal naming the field.
pub fn validate(body: &Value) -> Result<(CardData, Value), Response> {
    let refuse =
        |code: &str, message: &str, param: &str| Err(http::invalid(code, message, Some(param)));
    let method = &body["payment_method"];
    if method["type"] != "card" {
        return refuse(
            "invalid_card",
            "payment_method.type must be card",
            "$.payment_method.type",
        );
    }
    if !matches!(
        method["card_number_type"].as_str(),
        Some("fpan" | "network_token")
    ) {
        return refuse(
            "invalid_card",
            "card_number_type is fpan or network_token",
            "$.payment_method.card_number_type",
        );
    }
    let number = method["number"].as_str().unwrap_or_default();
    let digits_only = !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit());
    if !digits_only || !(12..=19).contains(&number.len()) {
        return refuse(
            "invalid_card",
            "number is 12 to 19 digits",
            "$.payment_method.number",
        );
    }
    if method["card_number_type"] == "fpan" && !luhn(number) {
        return refuse(
            "invalid_card",
            "the card number fails the Luhn check",
            "$.payment_method.number",
        );
    }
    let field = |name: &str| method.get(name).filter(|v| !v.is_null());
    if let Some(month) = field("exp_month") {
        let ok = month
            .as_str()
            .is_some_and(|m| m.len() <= 2 && m.parse::<u32>().is_ok_and(|m| (1..=12).contains(&m)));
        if !ok {
            return refuse(
                "invalid_card",
                "exp_month is 01 to 12",
                "$.payment_method.exp_month",
            );
        }
    }
    if let Some(year) = field("exp_year") {
        let ok = year
            .as_str()
            .is_some_and(|y| y.len() == 4 && y.bytes().all(|b| b.is_ascii_digit()));
        if !ok {
            return refuse(
                "invalid_card",
                "exp_year is four digits",
                "$.payment_method.exp_year",
            );
        }
    }
    if let Some(cvc) = field("cvc") {
        let ok = cvc
            .as_str()
            .is_some_and(|c| (3..=4).contains(&c.len()) && c.bytes().all(|b| b.is_ascii_digit()));
        if !ok {
            return refuse(
                "invalid_card",
                "cvc is 3 or 4 digits",
                "$.payment_method.cvc",
            );
        }
    }
    if field("iin").is_some_and(|i| i.as_str().map_or(true, |i| i.len() > 8)) {
        return refuse(
            "invalid_card",
            "iin is at most 8 characters",
            "$.payment_method.iin",
        );
    }
    if field("display_last4").is_some_and(|l| {
        l.as_str().map_or(true, |l| {
            l.len() != 4 || !l.bytes().all(|b| b.is_ascii_digit())
        })
    }) {
        return refuse(
            "invalid_card",
            "display_last4 is four digits",
            "$.payment_method.display_last4",
        );
    }
    if !matches!(
        method["display_card_funding_type"].as_str(),
        Some("credit" | "debit" | "prepaid")
    ) {
        return refuse(
            "invalid_card",
            "display_card_funding_type is credit, debit or prepaid",
            "$.payment_method.display_card_funding_type",
        );
    }
    if !method["metadata"].is_object() {
        return refuse(
            "invalid_card",
            "payment_method.metadata is an object",
            "$.payment_method.metadata",
        );
    }
    if let Some(checks) = field("checks_performed") {
        let ok = checks.as_array().is_some_and(|c| {
            c.iter()
                .all(|c| matches!(c.as_str(), Some("avs" | "cvv" | "ani" | "auth0")))
        });
        if !ok {
            return refuse(
                "invalid_card",
                "checks_performed holds avs, cvv, ani or auth0",
                "$.payment_method.checks_performed",
            );
        }
    }
    let allowance = &body["allowance"];
    if allowance["reason"] != "one_time" {
        return refuse(
            "invalid",
            "allowance.reason must be one_time",
            "$.allowance.reason",
        );
    }
    if !allowance["max_amount"].as_u64().is_some_and(|m| m > 0) {
        return refuse(
            "invalid",
            "allowance.max_amount is a positive integer of minor units",
            "$.allowance.max_amount",
        );
    }
    let currency = allowance["currency"].as_str().unwrap_or_default();
    if currency.len() != 3 || !currency.bytes().all(|b| b.is_ascii_lowercase()) {
        return refuse(
            "invalid",
            "allowance.currency is lowercase ISO 4217 (usd)",
            "$.allowance.currency",
        );
    }
    if allowance["checkout_session_id"]
        .as_str()
        .map_or(true, str::is_empty)
    {
        return refuse(
            "invalid",
            "allowance.checkout_session_id is required",
            "$.allowance.checkout_session_id",
        );
    }
    let merchant = allowance["merchant_id"].as_str().unwrap_or_default();
    if merchant.is_empty() || merchant.len() > 256 {
        return refuse(
            "invalid",
            "allowance.merchant_id is 1 to 256 characters",
            "$.allowance.merchant_id",
        );
    }
    let Some(expires) = allowance["expires_at"].as_str().and_then(http::seconds_of) else {
        return refuse(
            "invalid",
            "allowance.expires_at is RFC 3339",
            "$.allowance.expires_at",
        );
    };
    if expires <= http::now() {
        return refuse(
            "invalid",
            "allowance.expires_at is in the past",
            "$.allowance.expires_at",
        );
    }
    let signals = body["risk_signals"].as_array().cloned().unwrap_or_default();
    if signals.is_empty() {
        return refuse(
            "invalid",
            "at least one risk signal is required",
            "$.risk_signals",
        );
    }
    for (index, signal) in signals.iter().enumerate() {
        let ok = signal["type"] == "card_testing"
            && signal["score"].is_i64()
            && matches!(
                signal["action"].as_str(),
                Some("blocked" | "manual_review" | "authorized")
            );
        if !ok {
            return refuse(
                "invalid",
                "a risk signal is {type: card_testing, score, action}",
                &format!("$.risk_signals[{index}]"),
            );
        }
    }
    if !body["metadata"].is_object() {
        return refuse("invalid", "metadata is an object", "$.metadata");
    }
    if let Some(address) = body.get("billing_address").filter(|a| !a.is_null()) {
        for required in [
            "name",
            "line_one",
            "city",
            "state",
            "country",
            "postal_code",
        ] {
            if address[required].as_str().map_or(true, str::is_empty) {
                return refuse(
                    "invalid",
                    &format!("billing_address.{required} is required"),
                    &format!("$.billing_address.{required}"),
                );
            }
        }
        if address["country"].as_str().map_or(true, |c| c.len() != 2) {
            return refuse(
                "invalid",
                "billing_address.country is ISO 3166-1 alpha-2",
                "$.billing_address.country",
            );
        }
    }
    let text = |name: &str| method[name].as_str().map(str::to_string);
    Ok((
        CardData {
            number: number.to_string(),
            exp_month: text("exp_month"),
            exp_year: text("exp_year"),
            name: text("name"),
            cvc: text("cvc"),
        },
        allowance.clone(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn card_numbers_are_checked_and_never_printed() {
        assert!(luhn("4242424242424242"));
        assert!(!luhn("4242424242424241"));
        assert!(!luhn("42424242"));
        assert_eq!(brand_of("4242424242424242"), "visa");
        assert_eq!(brand_of("5555555555554444"), "mastercard");
        assert_eq!(brand_of("378282246310005"), "amex");
        let card = CardData {
            number: "4242424242424242".into(),
            exp_month: None,
            exp_year: None,
            name: None,
            cvc: Some("123".into()),
        };
        let printed = format!("{card:?}");
        assert!(
            !printed.contains("424242424242") && !printed.contains("123"),
            "{printed}"
        );
        assert!(printed.contains("4242"));
    }

    #[test]
    fn the_simulated_network_answers_by_test_card() {
        let card = |number: &str| CardData {
            number: number.into(),
            exp_month: None,
            exp_year: None,
            name: None,
            cvc: None,
        };
        let network = SimulatedAcquirer;
        assert!(network
            .authorize(&card("4242424242424242"), 100, "usd", "m", None)
            .is_ok());
        assert_eq!(
            network.authorize(&card("4000000000000002"), 100, "usd", "m", None),
            Err("card_declined".into())
        );
        assert_eq!(
            network.authorize(&card("4000000000003220"), 100, "usd", "m", None),
            Err("authentication_required".into())
        );
        let authenticated = json!({"outcome": "authenticated"});
        assert!(network
            .authorize(
                &card("4000000000003220"),
                100,
                "usd",
                "m",
                Some(&authenticated)
            )
            .is_ok());
        assert!(network.label().contains("simulated"));
    }

    #[test]
    fn a_delegate_request_is_validated_field_by_field() {
        let good = json!({
            "payment_method": {
                "type": "card", "card_number_type": "fpan", "virtual": false,
                "number": "4242424242424242", "exp_month": "11", "exp_year": "2030",
                "cvc": "123", "display_card_funding_type": "credit", "display_last4": "4242",
                "metadata": {},
            },
            "allowance": {
                "reason": "one_time", "max_amount": 2000, "currency": "usd",
                "checkout_session_id": "cs_1", "merchant_id": "m_1",
                "expires_at": http::rfc3339(http::now() + 600),
            },
            "risk_signals": [{"type": "card_testing", "score": 10, "action": "authorized"}],
            "metadata": {"source": "agent"},
        });
        assert!(validate(&good).is_ok());
        for (path, value, param) in [
            (
                ("payment_method", "number"),
                json!("4242424242424241"),
                "$.payment_method.number",
            ),
            (
                ("payment_method", "exp_month"),
                json!("13"),
                "$.payment_method.exp_month",
            ),
            (
                ("payment_method", "display_card_funding_type"),
                json!("charge"),
                "$.payment_method.display_card_funding_type",
            ),
            (
                ("allowance", "currency"),
                json!("USD"),
                "$.allowance.currency",
            ),
            (
                ("allowance", "reason"),
                json!("recurring"),
                "$.allowance.reason",
            ),
            (
                ("allowance", "expires_at"),
                json!("2020-01-01T00:00:00Z"),
                "$.allowance.expires_at",
            ),
        ] {
            let mut bad = good.clone();
            bad[path.0][path.1] = value;
            let refused = validate(&bad).unwrap_err();
            assert_eq!(refused.body["param"], param, "{}", refused.body);
        }
        let mut no_signals = good.clone();
        no_signals["risk_signals"] = json!([]);
        assert_eq!(
            validate(&no_signals).unwrap_err().body["param"],
            "$.risk_signals"
        );
    }
}
