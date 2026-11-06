//! 3-D Secure for agents: ACP's Delegate Authentication API.
//!
//! When a merchant asks for 3-D Secure, the agent authenticates the buyer's
//! card with an authentication provider before it completes the checkout,
//! and brings the result (`authentication_result`: the outcome, the
//! cryptogram, the ECI, the directory server's transaction id). The API is
//! served as ACP has it (API version 2026-04-17, 3DS2, browser channel):
//!
//! - `POST /delegate_authentication` makes a session for a card, an amount
//!   and a merchant: `action_required` with a fingerprint action (the 3DS
//!   Method), `pending` (none needed), or `not_supported` (the card is not
//!   enrolled).
//! - `POST /delegate_authentication/{id}/authenticate` with the
//!   fingerprint's completion (`Y`, `N`, `U`): `authenticated`
//!   (frictionless), `action_required` with a challenge, `attempted`,
//!   `not_authenticated`, `rejected`, `unavailable` or `expired`.
//! - `GET /delegate_authentication/{id}`: its state and, once there is
//!   one, the result.
//!
//! with ACP's rules: a bearer token, `API-Version`, a signature over the
//! canonical body (which ACP requires here, so an unsigned request is
//! refused unless a client is let off), idempotency on create (409 on a
//! conflict, as this RFC says), the flat errors and their JSONPath params.
//!
//! **The issuer is simulated.** Authenticating a card for real takes an
//! EMVCo-certified 3DS server connected to the card networks' directory
//! servers and the issuers' ACSs; none of that is built. The provider here
//! answers for a [`SimulatedAcs`]: it serves the 3DS Method and a challenge
//! page itself (`/acs/method`, `/acs/challenge`), decides by test card
//! number, and makes a random cryptogram that no network would honour. It
//! is for devnets and tests, and its results say so ([`SIMULATED_NOTICE`],
//! in `cardholder_info`).

use std::collections::BTreeMap;
use std::sync::Mutex;

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use serde_json::{json, Value};
use zeroize::Zeroizing;

use super::http::{self, Gate, Keys, Request, Response, Service};

/// The 3DS version the simulated ACS speaks.
pub const MESSAGE_VERSION: &str = "2.2.0";

/// How long an authentication session lives, in seconds.
pub const SESSION_FOR: u64 = 600;

/// The one-time code the simulated issuer's challenge takes.
pub const SIMULATED_OTP: &str = "123456";

/// What every result of the simulated issuer tells the cardholder.
pub const SIMULATED_NOTICE: &str = "Simulated issuer: no card network or bank was involved.";

/// What the simulated issuer does with a card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Behaviour {
    /// Not enrolled: create answers `not_supported`.
    NotEnrolled,
    /// No 3DS Method; authenticated without a challenge.
    Frictionless,
    /// A 3DS Method, then a challenge.
    Challenge,
    /// A 3DS Method, then rejected by the issuer.
    Rejected,
    /// A 3DS Method, then authenticated without a challenge (unless a
    /// challenge is mandated).
    MethodThenFrictionless,
}

/// The simulated issuer's access control server: it decides by test card
/// number.
///
/// | Card ends | Behaviour |
/// | --- | --- |
/// | `…0002` | not enrolled |
/// | `4000000000003063` | frictionless, no 3DS Method |
/// | `4000000000003220` | 3DS Method, then a challenge (code `123456`) |
/// | `4000000000001091` | rejected |
/// | any other card | 3DS Method, then frictionless |
#[derive(Clone, Copy, Debug, Default)]
pub struct SimulatedAcs;

impl SimulatedAcs {
    /// What it does with `number`.
    pub fn behaviour(&self, number: &str) -> Behaviour {
        match number {
            n if n.ends_with("0002") => Behaviour::NotEnrolled,
            "4000000000003063" => Behaviour::Frictionless,
            "4000000000003220" => Behaviour::Challenge,
            "4000000000001091" => Behaviour::Rejected,
            _ => Behaviour::MethodThenFrictionless,
        }
    }
}

struct Session {
    id: String,
    server_trans_id: String,
    acs_trans_id: String,
    ds_trans_id: String,
    number: Zeroizing<String>,
    directory: &'static str,
    challenge_mandated: bool,
    notification_url: Option<String>,
    status: &'static str,
    result: Option<Value>,
    expires: u64,
}

#[derive(Default)]
struct Book {
    keys: Keys,
    sessions: BTreeMap<String, Session>,
}

/// The delegate authentication provider.
pub struct ThreeDs {
    gate: Gate,
    acs: SimulatedAcs,
    base_url: String,
    book: Mutex<Book>,
}

impl std::fmt::Debug for ThreeDs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ThreeDs({}, simulated issuer)", self.base_url)
    }
}

fn directory_of(number: &str) -> &'static str {
    match super::psp::brand_of(number) {
        "mastercard" => "mastercard",
        "amex" => "american_express",
        _ => "visa",
    }
}

/// The ECI a directory server gives for a full authentication, or an
/// attempt.
fn eci(directory: &str, authenticated: bool) -> &'static str {
    match (directory, authenticated) {
        ("mastercard", true) => "02",
        ("mastercard", false) => "01",
        (_, true) => "05",
        (_, false) => "06",
    }
}

fn uuid() -> String {
    let hex = http::random_hex(16);
    format!(
        "{}-{}-4{}-a{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[13..16],
        &hex[17..20],
        &hex[20..32]
    )
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// A page that posts `fields` to `action` as soon as it loads (how an ACS
/// hands a result back to the agent's page).
fn autopost(action: &str, fields: &[(&str, &str)]) -> String {
    let inputs: String = fields
        .iter()
        .map(|(name, value)| {
            format!(
                r#"<input type="hidden" name="{}" value="{}">"#,
                escape(name),
                escape(value)
            )
        })
        .collect();
    format!(
        r#"<!doctype html><meta charset="utf-8"><title>3-D Secure</title><body onload="document.forms[0].submit()"><form method="post" action="{}">{inputs}<noscript><button>Continue</button></noscript></form><p>Simulated issuer: returning to the merchant…</p></body>"#,
        escape(action)
    )
}

/// The fields of a form body.
fn form_fields(body: &str) -> BTreeMap<String, String> {
    fn decode(text: &str) -> String {
        let bytes = text.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'+' => out.push(b' '),
                b'%' if i + 2 < bytes.len() => {
                    match u8::from_str_radix(text.get(i + 1..i + 3).unwrap_or(""), 16) {
                        Ok(byte) => {
                            out.push(byte);
                            i += 2;
                        }
                        Err(_) => out.push(b'%'),
                    }
                }
                byte => out.push(byte),
            }
            i += 1;
        }
        String::from_utf8_lossy(&out).into_owned()
    }
    body.split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(k, v)| (decode(k), decode(v)))
        .collect()
}

fn json_of_base64(text: &str) -> Option<Value> {
    let bytes = URL_SAFE_NO_PAD
        .decode(text.trim().trim_end_matches('='))
        .ok()
        .or_else(|| STANDARD.decode(text.trim()).ok())?;
    serde_json::from_slice(&bytes).ok()
}

impl ThreeDs {
    /// A provider at `base_url` (where its ACS pages are reached) for
    /// `clients`, its issuer simulated.
    pub fn new(
        base_url: &str,
        clients: Vec<http::Client>,
    ) -> Result<ThreeDs, crate::error::CliError> {
        Ok(ThreeDs {
            gate: Gate::new(clients, &[super::API_VERSION, "2026-01-28"])?,
            acs: SimulatedAcs,
            base_url: base_url.trim_end_matches('/').to_string(),
            book: Mutex::new(Book::default()),
        })
    }

    fn view(&self, session: &Session, with_result: bool) -> Value {
        let mut view = json!({
            "authentication_session_id": session.id,
            "status": session.status,
        });
        if with_result {
            if let Some(result) = &session.result {
                view["authentication_result"] = result.clone();
            }
        }
        view
    }

    fn fingerprint_action(&self, session: &Session) -> Value {
        json!({
            "type": "fingerprint",
            "fingerprint": {
                "three_ds_method_url": format!("{}/acs/method", self.base_url),
                "three_ds_server_trans_id": session.server_trans_id,
            }
        })
    }

    fn challenge_action(&self, session: &Session) -> Value {
        json!({
            "type": "challenge",
            "challenge": {
                "acs_url": format!("{}/acs/challenge", self.base_url),
                "acs_trans_id": session.acs_trans_id,
                "three_ds_server_trans_id": session.server_trans_id,
                "message_version": MESSAGE_VERSION,
            }
        })
    }

    fn result(session: &Session, trans_status: &str, reason: Option<&str>) -> Value {
        let mut result = json!({
            "trans_status": trans_status,
            "transaction_id": session.ds_trans_id,
            "three_ds_server_trans_id": session.server_trans_id,
            "version": MESSAGE_VERSION,
            // ACP's result has no field for it: the message to the
            // cardholder says the issuer was simulated.
            "cardholder_info": SIMULATED_NOTICE,
        });
        if matches!(trans_status, "Y" | "A") {
            let mut cryptogram = [0u8; 20];
            use aws_lc_rs::rand::SecureRandom;
            aws_lc_rs::rand::SystemRandom::new()
                .fill(&mut cryptogram)
                .expect("the system has randomness");
            result["three_ds_cryptogram"] = json!(STANDARD.encode(cryptogram));
            result["electronic_commerce_indicator"] =
                json!(eci(session.directory, trans_status == "Y"));
        }
        if let Some(reason) = reason {
            result["trans_status_reason"] = json!(reason);
        }
        result
    }

    fn create(&self, body: &Value) -> Response {
        let refuse =
            |code: &str, message: &str, param: &str| http::invalid(code, message, Some(param));
        let merchant = body["merchant_id"].as_str().unwrap_or_default();
        if merchant.is_empty() {
            return refuse("invalid", "merchant_id is required", "$.merchant_id");
        }
        let method = &body["payment_method"];
        let number = method["number"].as_str().unwrap_or_default();
        if method["type"] != "card" {
            return refuse(
                "invalid_card",
                "payment_method.type must be card",
                "$.payment_method.type",
            );
        }
        if !super::psp::luhn(number) {
            return refuse(
                "invalid_card",
                "the card number is not a valid card number",
                "$.payment_method.number",
            );
        }
        let month_ok = method["exp_month"]
            .as_str()
            .is_some_and(|m| m.len() <= 2 && m.parse::<u32>().is_ok_and(|m| (1..=12).contains(&m)));
        if !month_ok {
            return refuse(
                "invalid_card",
                "exp_month is 01 to 12",
                "$.payment_method.exp_month",
            );
        }
        if !method["exp_year"]
            .as_str()
            .is_some_and(|y| y.len() == 4 && y.bytes().all(|b| b.is_ascii_digit()))
        {
            return refuse(
                "invalid_card",
                "exp_year is four digits",
                "$.payment_method.exp_year",
            );
        }
        if method["name"].as_str().map_or(true, str::is_empty) {
            return refuse(
                "invalid_card",
                "payment_method.name is required",
                "$.payment_method.name",
            );
        }
        if !body["amount"]["value"].as_u64().is_some_and(|v| v > 0) {
            return refuse(
                "invalid",
                "amount.value is a positive integer of minor units",
                "$.amount.value",
            );
        }
        let currency = body["amount"]["currency"].as_str().unwrap_or_default();
        if currency.len() != 3 || !currency.bytes().all(|b| b.is_ascii_uppercase()) {
            return refuse(
                "invalid",
                "amount.currency is ISO 4217, upper case (USD)",
                "$.amount.currency",
            );
        }
        if let Some(channel) = body.get("channel").filter(|c| !c.is_null()) {
            if let Err((message, param)) = check_channel(channel) {
                return refuse("invalid", &message, &param);
            }
        }
        if let Some(details) = body.get("acquirer_details").filter(|d| !d.is_null()) {
            for (field, most) in [
                ("acquirer_bin", 11),
                ("acquirer_merchant_id", 35),
                ("merchant_name", 40),
                ("requestor_id", 35),
            ] {
                if details[field].as_str().is_some_and(|v| v.len() > most) {
                    return refuse(
                        "invalid",
                        &format!("acquirer_details.{field} is at most {most} characters"),
                        &format!("$.acquirer_details.{field}"),
                    );
                }
            }
            if details["acquirer_country"]
                .as_str()
                .is_some_and(|c| c.len() != 2)
            {
                return refuse(
                    "invalid",
                    "acquirer_details.acquirer_country is ISO 3166-1 alpha-2",
                    "$.acquirer_details.acquirer_country",
                );
            }
        }
        let preference = &body["flow_preference"];
        if !preference.is_null()
            && !matches!(
                preference["type"].as_str(),
                Some("challenge" | "frictionless")
            )
        {
            return refuse(
                "invalid",
                "flow_preference.type is challenge or frictionless",
                "$.flow_preference.type",
            );
        }
        let behaviour = self.acs.behaviour(number);
        let mut session = Session {
            id: http::fresh_id("auth"),
            server_trans_id: uuid(),
            acs_trans_id: String::new(),
            ds_trans_id: uuid(),
            number: Zeroizing::new(number.to_string()),
            directory: directory_of(number),
            challenge_mandated: preference["type"] == "challenge"
                && preference["challenge"]["type"] == "mandated",
            notification_url: body["challenge_notification_url"]
                .as_str()
                .map(str::to_string),
            status: "pending",
            result: None,
            expires: http::now() + SESSION_FOR,
        };
        let mut view;
        match behaviour {
            Behaviour::NotEnrolled => {
                session.status = "not_supported";
                view = self.view(&session, false);
            }
            Behaviour::Frictionless => {
                view = self.view(&session, false);
            }
            _ => {
                session.status = "action_required";
                view = self.view(&session, false);
                view["action"] = self.fingerprint_action(&session);
            }
        }
        self.book
            .lock()
            .expect("3ds lock")
            .sessions
            .insert(session.id.clone(), session);
        Response::json(201, view)
    }

    fn authenticate(&self, id: &str, body: &Value) -> Response {
        let completion = body["fingerprint_completion"].as_str().unwrap_or_default();
        if !matches!(completion, "Y" | "N" | "U") {
            return http::invalid(
                "invalid",
                "fingerprint_completion is Y, N or U",
                Some("$.fingerprint_completion"),
            );
        }
        if let Some(channel) = body.get("channel").filter(|c| !c.is_null()) {
            if let Err((message, param)) = check_channel(channel) {
                return http::invalid("invalid", &message, Some(&param));
            }
        }
        let mut book = self.book.lock().expect("3ds lock");
        let Some(session) = book.sessions.get_mut(id) else {
            return http::not_found("no such authentication session");
        };
        if http::now() > session.expires {
            session.status = "expired";
            return Response::json(200, self.view(session, false));
        }
        if !matches!(session.status, "action_required" | "pending")
            || !session.acs_trans_id.is_empty()
        {
            return http::error(
                409,
                "invalid_request",
                "invalid_state",
                &format!("the session is {}", session.status),
                None,
            );
        }
        if let Some(url) = body["challenge_notification_url"].as_str() {
            session.notification_url = Some(url.to_string());
        }
        let behaviour = self.acs.behaviour(&session.number);
        let challenge = behaviour == Behaviour::Challenge
            || (session.challenge_mandated && behaviour != Behaviour::Rejected);
        let mut view;
        if behaviour == Behaviour::Rejected {
            session.status = "rejected";
            session.result = Some(Self::result(session, "R", Some("11")));
            view = self.view(session, false);
        } else if challenge {
            if session.notification_url.is_none() {
                return http::invalid(
                    "invalid",
                    "challenge_notification_url is required for a challenge",
                    Some("$.challenge_notification_url"),
                );
            }
            session.acs_trans_id = uuid();
            session.status = "action_required";
            view = self.view(session, false);
            view["action"] = self.challenge_action(session);
        } else {
            session.acs_trans_id = uuid();
            session.status = "authenticated";
            session.result = Some(Self::result(session, "Y", None));
            view = self.view(session, false);
        }
        Response::json(200, view)
    }

    fn retrieve(&self, id: &str) -> Response {
        let mut book = self.book.lock().expect("3ds lock");
        let Some(session) = book.sessions.get_mut(id) else {
            return http::not_found("no such authentication session");
        };
        if http::now() > session.expires && session.result.is_none() {
            session.status = "expired";
        }
        Response::json(200, self.view(session, true))
    }

    /// The simulated issuer's 3DS Method: it notes the fingerprint and posts
    /// back to the agent's notification URL.
    fn method(&self, body: &str) -> Response {
        let fields = form_fields(body);
        let Some(data) = fields
            .get("threeDSMethodData")
            .and_then(|d| json_of_base64(d))
        else {
            return Response::page(400, "<p>threeDSMethodData is missing.</p>".into());
        };
        let server_trans_id = data["threeDSServerTransID"].as_str().unwrap_or_default();
        let known = self
            .book
            .lock()
            .expect("3ds lock")
            .sessions
            .values()
            .any(|s| s.server_trans_id == server_trans_id);
        if !known {
            return Response::page(404, "<p>No such transaction.</p>".into());
        }
        let back = json!({"threeDSServerTransID": server_trans_id}).to_string();
        match data["threeDSMethodNotificationURL"].as_str() {
            Some(url) => Response::page(
                200,
                autopost(url, &[("threeDSMethodData", &URL_SAFE_NO_PAD.encode(back))]),
            ),
            None => Response::page(200, "<p>Fingerprint taken.</p>".into()),
        }
    }

    /// The simulated issuer's challenge page, for the CReq the agent posts.
    fn challenge_page(&self, body: &str) -> Response {
        let fields = form_fields(body);
        let Some(creq) = fields.get("creq").and_then(|c| json_of_base64(c)) else {
            return Response::page(400, "<p>creq is missing.</p>".into());
        };
        let acs_trans_id = creq["acsTransID"].as_str().unwrap_or_default();
        let known = self
            .book
            .lock()
            .expect("3ds lock")
            .sessions
            .values()
            .any(|s| s.acs_trans_id == acs_trans_id && s.status == "action_required");
        if !known || creq["messageType"] != "CReq" {
            return Response::page(404, "<p>No such challenge.</p>".into());
        }
        Response::page(
            200,
            format!(
                r#"<!doctype html><meta charset="utf-8"><title>Simulated issuer</title><form method="post" action="{}/acs/challenge/submit"><input type="hidden" name="acsTransID" value="{}"><p>Simulated issuer: no bank is involved. Enter {SIMULATED_OTP}.</p><input name="otp" autocomplete="one-time-code" inputmode="numeric"><button>Submit</button></form>"#,
                escape(&self.base_url),
                escape(acs_trans_id)
            ),
        )
    }

    /// The challenge's answer: the result is decided, and the CRes posted to
    /// the agent's notification URL.
    fn challenge_submit(&self, body: &str) -> Response {
        let fields = form_fields(body);
        let acs_trans_id = fields.get("acsTransID").cloned().unwrap_or_default();
        let passed = fields
            .get("otp")
            .is_some_and(|otp| http::same_secret(otp.trim(), SIMULATED_OTP));
        let mut book = self.book.lock().expect("3ds lock");
        let Some(session) = book
            .sessions
            .values_mut()
            .find(|s| s.acs_trans_id == acs_trans_id && s.status == "action_required")
        else {
            return Response::page(404, "<p>No such challenge.</p>".into());
        };
        let status = if passed { "Y" } else { "N" };
        session.status = if passed {
            "authenticated"
        } else {
            "not_authenticated"
        };
        session.result = Some(Self::result(session, status, (!passed).then_some("01")));
        let cres = json!({
            "threeDSServerTransID": session.server_trans_id,
            "acsTransID": session.acs_trans_id,
            "messageType": "CRes",
            "messageVersion": MESSAGE_VERSION,
            "transStatus": status,
        })
        .to_string();
        match &session.notification_url {
            Some(url) => Response::page(
                200,
                autopost(url, &[("cres", &URL_SAFE_NO_PAD.encode(cres))]),
            ),
            None => Response::page(200, "<p>Done.</p>".into()),
        }
    }
}

/// Checks a browser channel as ACP's validation rules have it.
fn check_channel(channel: &Value) -> Result<(), (String, String)> {
    if channel["type"] != "browser" {
        return Err((
            "channel.type must be browser".into(),
            "$.channel.type".into(),
        ));
    }
    let browser = &channel["browser"];
    for field in ["accept_header", "ip_address", "language", "user_agent"] {
        if browser[field].as_str().map_or(true, str::is_empty) {
            return Err((
                format!("channel.browser.{field} is required"),
                format!("$.channel.browser.{field}"),
            ));
        }
    }
    let Some(javascript) = browser["javascript_enabled"].as_bool() else {
        return Err((
            "channel.browser.javascript_enabled is required".into(),
            "$.channel.browser.javascript_enabled".into(),
        ));
    };
    if javascript {
        for field in [
            "color_depth",
            "java_enabled",
            "screen_height",
            "screen_width",
            "timezone_offset",
        ] {
            if browser[field].is_null() {
                return Err((
                    format!("channel.browser.{field} is required when JavaScript is enabled"),
                    format!("$.channel.browser.{field}"),
                ));
            }
        }
    }
    Ok(())
}

impl Service for ThreeDs {
    fn answer(&self, request: &Request<'_>) -> Response {
        // The ACS's pages are for the buyer's browser: no token.
        match (request.method, request.path) {
            ("POST", "/acs/method") => return self.method(request.body),
            ("POST", "/acs/challenge") => return self.challenge_page(request.body),
            ("POST", "/acs/challenge/submit") => return self.challenge_submit(request.body),
            _ => {}
        }
        let client = match self.gate.admit(request) {
            Ok(client) => client.name.clone(),
            Err(refused) => return http::echo(request, refused),
        };
        let parts: Vec<&str> = request.path.split('/').filter(|p| !p.is_empty()).collect();
        let answer = match (request.method, parts.as_slice()) {
            ("POST", ["delegate_authentication"]) => {
                self.keyed(request, &client, |body| self.create(body))
            }
            ("POST", ["delegate_authentication", id, "authenticate"]) => {
                self.keyed(request, &client, |body| self.authenticate(id, body))
            }
            ("GET", ["delegate_authentication", id]) => self.retrieve(id),
            _ => http::not_found("no such endpoint"),
        };
        http::echo(request, answer)
    }

    fn tick(&self) {
        // Sessions an hour past their end go, and their card numbers.
        let now = http::now();
        self.book
            .lock()
            .expect("3ds lock")
            .sessions
            .retain(|_, s| now < s.expires + 3600);
    }
}

impl ThreeDs {
    /// Runs `then` under the request's idempotency key, if it has one (ACP
    /// recommends one here): a conflict is a 409, as this RFC has it.
    fn keyed(
        &self,
        request: &Request<'_>,
        client: &str,
        then: impl FnOnce(&Value) -> Response,
    ) -> Response {
        let body = match http::body_of(request) {
            Ok(body) => body,
            Err(refused) => return refused,
        };
        let key = match http::key_of(request, false) {
            Ok(key) => key,
            Err(refused) => return refused,
        };
        let Some(key) = key else {
            return then(&body);
        };
        let slot = {
            let mut book = self.book.lock().expect("3ds lock");
            match http::claim(&mut book.keys, 409, client, request.path, key, &body) {
                Ok(slot) => slot,
                Err(answered) => return answered,
            }
        };
        let response = then(&body);
        http::settle(
            &mut self.book.lock().expect("3ds lock").keys,
            slot,
            &response,
        );
        response
    }
}

/// ACP's checkout `authentication_result` for a retrieved session: the
/// outcome its `trans_status` stands for, with the details an authenticated
/// result carries.
pub fn checkout_result(session: &Value) -> Value {
    let result = &session["authentication_result"];
    let outcome = match (session["status"].as_str(), result["trans_status"].as_str()) {
        (Some("not_supported"), _) => "not_supported",
        (_, Some("Y")) => "authenticated",
        (_, Some("A")) => "attempt_acknowledged",
        (_, Some("I")) => "informational",
        (_, Some("N")) => "denied",
        (_, Some("R")) => "rejected",
        (Some("challenge_abandoned"), _) => "abandoned",
        (Some("expired"), _) => "canceled",
        _ => "internal_error",
    };
    let mut checkout = json!({"outcome": outcome});
    if matches!(
        outcome,
        "authenticated" | "attempt_acknowledged" | "informational"
    ) {
        checkout["outcome_details"] = json!({
            "three_ds_cryptogram": result["three_ds_cryptogram"],
            "electronic_commerce_indicator": result["electronic_commerce_indicator"],
            "transaction_id": result["transaction_id"],
            "version": result["version"],
        });
    }
    checkout
}

/// The `threeDSMethodData` an agent's page posts to the 3DS Method URL.
pub fn method_data(server_trans_id: &str, notification_url: &str) -> String {
    URL_SAFE_NO_PAD.encode(
        json!({
            "threeDSServerTransID": server_trans_id,
            "threeDSMethodNotificationURL": notification_url,
        })
        .to_string(),
    )
}

/// The `creq` an agent's page posts to a challenge's ACS URL.
pub fn creq(challenge: &Value) -> String {
    URL_SAFE_NO_PAD.encode(
        json!({
            "threeDSServerTransID": challenge["three_ds_server_trans_id"],
            "acsTransID": challenge["acs_trans_id"],
            "messageVersion": challenge["message_version"],
            "messageType": "CReq",
            "challengeWindowSize": "05",
        })
        .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_form_body_is_read_back() {
        let fields = form_fields("creq=eyJh%3D&otp=12+34");
        assert_eq!(fields["creq"], "eyJh=");
        assert_eq!(fields["otp"], "12 34");
    }

    #[test]
    fn the_simulated_issuer_decides_by_card() {
        let acs = SimulatedAcs;
        assert_eq!(acs.behaviour("4000000000000002"), Behaviour::NotEnrolled);
        assert_eq!(acs.behaviour("4000000000003220"), Behaviour::Challenge);
        assert_eq!(
            acs.behaviour("4242424242424242"),
            Behaviour::MethodThenFrictionless
        );
        assert_eq!(eci("visa", true), "05");
        assert_eq!(eci("mastercard", true), "02");
    }

    #[test]
    fn a_result_maps_to_acps_checkout_outcome() {
        let authenticated = json!({
            "status": "authenticated",
            "authentication_result": {
                "trans_status": "Y",
                "three_ds_cryptogram": STANDARD.encode([7u8; 20]),
                "electronic_commerce_indicator": "05",
                "transaction_id": "ds-1",
                "version": MESSAGE_VERSION,
            }
        });
        let checkout = checkout_result(&authenticated);
        assert_eq!(checkout["outcome"], "authenticated");
        assert!(super::super::card::check_authentication(&checkout).is_ok());
        assert_eq!(
            checkout_result(&json!({"status": "not_supported"}))["outcome"],
            "not_supported"
        );
        assert_eq!(
            checkout_result(
                &json!({"status": "rejected", "authentication_result": {"trans_status": "R"}})
            )["outcome"],
            "rejected"
        );
    }

    #[test]
    fn pages_escape_what_they_are_given() {
        let page = autopost("https://agent.example/cb?a=1&b=\"2\"", &[("cres", "<x>")]);
        assert!(page.contains("&amp;b=&quot;2&quot;") && page.contains("&lt;x&gt;"));
    }
}
