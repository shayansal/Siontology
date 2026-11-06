//! Cards at the merchant: ACP's `dev.acp.tokenized.card` payment handler.
//!
//! An agent that pays by card first delegates the card to the merchant's
//! PSP (`POST /agentic_commerce/delegate_payment`, scoped by an allowance
//! to this checkout, this merchant and at most its total), then completes
//! the session with an instrument naming this handler and the token the PSP
//! gave: `{"type": "card", "credential": {"type": "spt", "token": …}}`. The
//! merchant charges the token through its PSP ([`Processor`]):
//!
//! - **Stripe** ([`Stripe`]): the token is a Stripe shared payment token,
//!   charged by creating a PaymentIntent with
//!   `payment_method_data[shared_payment_granted_token]` and
//!   `confirm=true`, as Stripe documents it for sellers (API version
//!   `2026-09-30.preview`), with the merchant's own secret key. A 3-D
//!   Secure result the agent brings is passed in
//!   `payment_method_options[card][three_d_secure]`. Built to Stripe's
//!   documentation and tested against a stand-in for its API here: it was
//!   not run against Stripe itself.
//! - **An ACP PSP** ([`PspProcessor`]): a PSP that serves ACP's delegate
//!   payment API and redeems its vault tokens with `POST
//!   /agentic_commerce/create_and_process_payment_intent`, the shape of
//!   NVIDIA's ACP reference implementation (ACP leaves redemption to the
//!   PSP); [`super::psp`] serves it.
//!
//! When the merchant states 3-D Secure (`authentication_metadata`), a card
//! completion without an acceptable `authentication_result` leaves the
//! session `authentication_required`, and nothing is charged.
//!
//! A card payment is the PSP's word: unlike a ledger payment, nothing here
//! can check it (trust atom `agent:acp-card`).

use serde_json::{json, Value};

use super::http;
use crate::error::CliError;

/// ACP's card handler.
pub const HANDLER: &str = "dev.acp.tokenized.card";
/// The version of the card handler served.
pub const HANDLER_VERSION: &str = "2026-01-22";
/// The card handler's id within a session.
pub const HANDLER_ID: &str = "card_tokenized";
/// Where the card handler is specified.
pub const HANDLER_SPEC: &str = "https://github.com/agentic-commerce-protocol/agentic-commerce-protocol/blob/main/rfcs/rfc.payment_handlers.md";

/// The Stripe API version shared payment tokens are charged with.
pub const STRIPE_VERSION: &str = "2026-09-30.preview";

/// A charge the PSP made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Charge {
    /// The PSP's id for it (a PaymentIntent, a payment intent).
    pub id: String,
    /// Its status, as the PSP says it.
    pub status: String,
}

/// Why a card was not charged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Declined {
    /// The card was declined.
    Card(String),
    /// The issuer wants the buyer authenticated (3-D Secure).
    NeedsAuthentication(String),
    /// The PSP could not be reached, or failed.
    Failed(String),
}

impl Declined {
    /// What happened, for a message to the agent.
    pub fn describe(&self) -> &str {
        match self {
            Declined::Card(why) | Declined::NeedsAuthentication(why) | Declined::Failed(why) => why,
        }
    }
}

/// What to charge.
#[derive(Clone, Debug)]
pub struct ChargeRequest<'a> {
    /// The delegated token.
    pub token: &'a str,
    /// How much, in minor units.
    pub amount: u64,
    /// In what (lowercase ISO 4217).
    pub currency: &'a str,
    /// The session paid for.
    pub checkout_session_id: &'a str,
    /// An idempotency key: the same charge asked twice is made once.
    pub idempotency_key: &'a str,
    /// The 3-D Secure result the agent brought (ACP's
    /// `AuthenticationResult`).
    pub authentication: Option<&'a Value>,
}

/// A PSP that charges delegated card tokens.
pub trait Processor: Send + Sync {
    /// The PSP, as the handler names it (`psp`).
    fn psp(&self) -> &str;

    /// Charges a token.
    fn charge(&self, request: &ChargeRequest<'_>) -> Result<Charge, Declined>;

    /// Refunds `amount` of a charge: the refund's id.
    fn refund(&self, charge: &str, amount: u64, key: &str) -> Result<String, String>;
}

/// The card handler a merchant declares.
pub struct CardHandler {
    /// Who charges the cards.
    pub processor: Box<dyn Processor>,
    /// The merchant's id at its PSP (a delegated token's allowance names
    /// it).
    pub merchant_id: String,
    /// The brands taken.
    pub accepted_brands: Vec<String>,
    /// 3-D Secure, if the merchant asks for it: ACP's
    /// `AuthenticationMetadata` (acquirer details and directory server).
    pub three_ds: Option<Value>,
}

impl std::fmt::Debug for CardHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "CardHandler({}, {})",
            self.processor.psp(),
            self.merchant_id
        )
    }
}

impl CardHandler {
    /// Its declaration in a session's `capabilities.payment.handlers`.
    pub fn declaration(&self) -> Value {
        json!({
            "id": HANDLER_ID,
            "name": HANDLER,
            "version": HANDLER_VERSION,
            "spec": HANDLER_SPEC,
            "requires_delegate_payment": true,
            "requires_pci_compliance": false,
            "psp": self.processor.psp(),
            "config_schema": "https://siontology.org/docs/agent/commerce/card-config.schema.json",
            "instrument_schemas": ["https://siontology.org/docs/agent/commerce/card-instrument.schema.json"],
            "config": {
                "merchant_id": self.merchant_id,
                "accepted_brands": self.accepted_brands,
                "accepted_funding_types": ["credit", "debit", "prepaid"],
                "psp": self.processor.psp(),
            },
            "display_order": 1,
        })
    }
}

/// The outcomes of 3-D Secure a merchant that asks for it takes: the buyer
/// was authenticated, an attempt was acknowledged, the result is only
/// informational, or the card is not enrolled (which ACP says the merchant
/// may go on without).
pub const ACCEPTED_OUTCOMES: [&str; 4] = [
    "authenticated",
    "attempt_acknowledged",
    "informational",
    "not_supported",
];

/// Checks an `authentication_result` against ACP's schema: an outcome it
/// defines, and the details it requires for the outcomes that carry them
/// (a cryptogram of 20 bytes in 28 base64 characters, an ECI it defines).
pub fn check_authentication(result: &Value) -> Result<&str, String> {
    const OUTCOMES: [&str; 10] = [
        "abandoned",
        "attempt_acknowledged",
        "authenticated",
        "canceled",
        "denied",
        "informational",
        "internal_error",
        "not_supported",
        "processing_error",
        "rejected",
    ];
    let outcome = result["outcome"]
        .as_str()
        .filter(|o| OUTCOMES.contains(o))
        .ok_or("authentication_result.outcome is one ACP defines")?;
    if matches!(
        outcome,
        "authenticated" | "informational" | "attempt_acknowledged"
    ) {
        let details = &result["outcome_details"];
        let cryptogram = details["three_ds_cryptogram"].as_str().unwrap_or_default();
        use base64::Engine;
        let decoded = base64::engine::general_purpose::STANDARD.decode(cryptogram);
        if cryptogram.len() != 28 || decoded.map(|d| d.len()) != Ok(20) {
            return Err("an authenticated result's cryptogram is 20 bytes, base64".into());
        }
        let eci = details["electronic_commerce_indicator"].as_str();
        if !matches!(eci, Some("01" | "02" | "05" | "06" | "07")) {
            return Err("an authenticated result's ECI is 01, 02, 05, 06 or 07".into());
        }
        if details["transaction_id"]
            .as_str()
            .map_or(true, str::is_empty)
        {
            return Err("an authenticated result names its directory server transaction".into());
        }
    }
    Ok(outcome)
}

/// The ARes `transStatus` an outcome stands for.
pub fn trans_status_of(outcome: &str) -> Option<&'static str> {
    Some(match outcome {
        "authenticated" => "Y",
        "attempt_acknowledged" => "A",
        "informational" => "I",
        "denied" => "N",
        "rejected" => "R",
        _ => return None,
    })
}

/// A form body (`application/x-www-form-urlencoded`).
pub fn form(fields: &[(String, String)]) -> String {
    fn encode(text: &str) -> String {
        let mut out = String::new();
        for byte in text.bytes() {
            match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    out.push(byte as char)
                }
                b' ' => out.push('+'),
                other => out.push_str(&format!("%{other:02X}")),
            }
        }
        out
    }
    fields
        .iter()
        .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

// ----- Stripe -------------------------------------------------------------

/// Stripe, charging shared payment tokens with the merchant's secret key.
pub struct Stripe {
    secret: String,
    base: String,
    client: sion_http::Client,
}

impl std::fmt::Debug for Stripe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The key is never printed.
        write!(f, "Stripe({})", self.base)
    }
}

impl Stripe {
    /// Stripe with `secret` (an `sk_` or `rk_` key), at `base`
    /// (`https://api.stripe.com` unless a stand-in is named).
    pub fn new(secret: &str, base: Option<&str>) -> Result<Stripe, CliError> {
        if !(secret.starts_with("sk_") || secret.starts_with("rk_")) {
            return Err(CliError::Usage(
                "a Stripe secret key starts with sk_ (or rk_ for a restricted key)".into(),
            ));
        }
        let base = base
            .unwrap_or("https://api.stripe.com")
            .trim_end_matches('/')
            .to_string();
        let local = !base.starts_with("https://api.stripe.com");
        Ok(Stripe {
            secret: secret.to_string(),
            client: http::outbound(local)?,
            base,
        })
    }

    fn post(
        &self,
        path: &str,
        fields: &[(String, String)],
        key: &str,
    ) -> Result<(u16, Value), String> {
        use base64::Engine;
        let authorization = format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(format!("{}:", self.secret))
        );
        let body = form(fields);
        http::call(
            &self.client,
            "POST",
            &format!("{}{path}", self.base),
            &[
                ("Authorization", authorization.as_str()),
                ("Stripe-Version", STRIPE_VERSION),
                ("Idempotency-Key", key),
                ("Content-Type", "application/x-www-form-urlencoded"),
            ],
            Some(body.as_bytes()),
        )
        .map_err(|e| e.to_string())
    }
}

impl Processor for Stripe {
    fn psp(&self) -> &str {
        "stripe"
    }

    fn charge(&self, request: &ChargeRequest<'_>) -> Result<Charge, Declined> {
        let mut fields = vec![
            ("amount".to_string(), request.amount.to_string()),
            ("currency".to_string(), request.currency.to_string()),
            (
                "payment_method_data[shared_payment_granted_token]".to_string(),
                request.token.to_string(),
            ),
            ("confirm".to_string(), "true".to_string()),
            (
                "metadata[checkout_session_id]".to_string(),
                request.checkout_session_id.to_string(),
            ),
        ];
        if let Some(result) = request.authentication {
            let details = &result["outcome_details"];
            let prefix = "payment_method_options[card][three_d_secure]";
            for (field, value) in [
                ("cryptogram", details["three_ds_cryptogram"].as_str()),
                (
                    "electronic_commerce_indicator",
                    details["electronic_commerce_indicator"].as_str(),
                ),
                ("transaction_id", details["transaction_id"].as_str()),
                ("version", details["version"].as_str()),
                (
                    "ares_trans_status",
                    result["outcome"].as_str().and_then(trans_status_of),
                ),
            ] {
                if let Some(value) = value {
                    fields.push((format!("{prefix}[{field}]"), value.to_string()));
                }
            }
        }
        let (status, body) = self
            .post("/v1/payment_intents", &fields, request.idempotency_key)
            .map_err(Declined::Failed)?;
        if !(200..300).contains(&status) {
            let error = &body["error"];
            let why = error["message"]
                .as_str()
                .unwrap_or("Stripe refused the payment")
                .to_string();
            return Err(match error["type"].as_str() {
                Some("card_error") if error["code"] == "authentication_required" => {
                    Declined::NeedsAuthentication(why)
                }
                Some("card_error") => Declined::Card(why),
                _ => Declined::Failed(format!("Stripe answered {status}: {why}")),
            });
        }
        let id = body["id"].as_str().unwrap_or_default().to_string();
        match body["status"].as_str() {
            Some("succeeded") => Ok(Charge {
                id,
                status: "succeeded".into(),
            }),
            Some("requires_action") => Err(Declined::NeedsAuthentication(
                "the issuer asks for the buyer to be authenticated".into(),
            )),
            Some(other) => Err(Declined::Card(format!(
                "the PaymentIntent is {other}, not succeeded"
            ))),
            None => Err(Declined::Failed("Stripe answered no status".into())),
        }
    }

    fn refund(&self, charge: &str, amount: u64, key: &str) -> Result<String, String> {
        let fields = vec![
            ("payment_intent".to_string(), charge.to_string()),
            ("amount".to_string(), amount.to_string()),
        ];
        let (status, body) = self.post("/v1/refunds", &fields, key)?;
        if (200..300).contains(&status) && body["status"] != "failed" {
            Ok(body["id"].as_str().unwrap_or_default().to_string())
        } else {
            Err(format!(
                "Stripe did not refund ({status}): {}",
                body["error"]["message"]
            ))
        }
    }
}

// ----- An ACP PSP ---------------------------------------------------------

/// A PSP serving ACP's delegate payment API, redeeming its vault tokens
/// with `create_and_process_payment_intent`.
pub struct PspProcessor {
    name: String,
    base: String,
    token: String,
    client: sion_http::Client,
}

impl std::fmt::Debug for PspProcessor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PspProcessor({}, {})", self.name, self.base)
    }
}

impl PspProcessor {
    /// The PSP `name` at `base`, the merchant known to it by `token`.
    pub fn new(name: &str, base: &str, token: &str) -> Result<PspProcessor, CliError> {
        Ok(PspProcessor {
            name: name.to_string(),
            base: base.trim_end_matches('/').to_string(),
            token: token.to_string(),
            client: http::outbound(true)?,
        })
    }
}

impl Processor for PspProcessor {
    fn psp(&self) -> &str {
        &self.name
    }

    fn charge(&self, request: &ChargeRequest<'_>) -> Result<Charge, Declined> {
        let mut body = json!({
            "vault_token": request.token,
            "amount": request.amount,
            "currency": request.currency,
        });
        if let Some(result) = request.authentication {
            body["authentication_result"] = result.clone();
        }
        let text = body.to_string();
        let bearer = format!("Bearer {}", self.token);
        let (status, answer) = http::call(
            &self.client,
            "POST",
            &format!(
                "{}/agentic_commerce/create_and_process_payment_intent",
                self.base
            ),
            &[
                ("Authorization", bearer.as_str()),
                ("Content-Type", "application/json"),
                ("API-Version", super::API_VERSION),
                ("Idempotency-Key", request.idempotency_key),
            ],
            Some(text.as_bytes()),
        )
        .map_err(|e| Declined::Failed(e.to_string()))?;
        let why = || {
            answer["message"]
                .as_str()
                .or_else(|| answer["detail"]["message"].as_str())
                .unwrap_or("the PSP refused the payment")
                .to_string()
        };
        match status {
            200..=299 if matches!(answer["status"].as_str(), Some("completed" | "succeeded")) => {
                Ok(Charge {
                    id: answer["id"].as_str().unwrap_or_default().to_string(),
                    status: answer["status"].as_str().unwrap_or_default().to_string(),
                })
            }
            200..=299 => Err(Declined::Card(format!(
                "the payment intent is {}",
                answer["status"]
            ))),
            402 if answer["code"] == "authentication_required" => {
                Err(Declined::NeedsAuthentication(why()))
            }
            400..=499 => Err(Declined::Card(why())),
            _ => Err(Declined::Failed(format!(
                "the PSP answered {status}: {}",
                why()
            ))),
        }
    }

    fn refund(&self, _charge: &str, _amount: u64, _key: &str) -> Result<String, String> {
        Err("this PSP's API has no refunds (ACP does not define one); refund it at the PSP".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_3ds_result_is_checked_against_acps_schema() {
        let good = json!({
            "outcome": "authenticated",
            "outcome_details": {
                "three_ds_cryptogram": "AAABBBCCCDDDEEEFFFGGGHHHIII=",
                "electronic_commerce_indicator": "05",
                "transaction_id": "ds-1",
                "version": "2.2.0",
            }
        });
        assert_eq!(check_authentication(&good), Ok("authenticated"));
        let mut short = good.clone();
        short["outcome_details"]["three_ds_cryptogram"] = json!("AAAA");
        assert!(check_authentication(&short).is_err());
        let mut eci = good.clone();
        eci["outcome_details"]["electronic_commerce_indicator"] = json!("99");
        assert!(check_authentication(&eci).is_err());
        assert_eq!(
            check_authentication(&json!({"outcome": "not_supported"})),
            Ok("not_supported")
        );
        assert!(check_authentication(&json!({"outcome": "sunny"})).is_err());
        assert_eq!(trans_status_of("authenticated"), Some("Y"));
    }

    #[test]
    fn a_form_is_percent_encoded() {
        assert_eq!(
            form(&[
                (
                    "payment_method_data[shared_payment_granted_token]".into(),
                    "spt_1".into()
                ),
                ("amount".into(), "1 000".into())
            ]),
            "payment_method_data%5Bshared_payment_granted_token%5D=spt_1&amount=1+000"
        );
    }

    #[test]
    fn a_stripe_key_is_a_secret_key() {
        assert!(Stripe::new("pk_test_123", None).is_err());
        let stripe = Stripe::new("sk_test_0123456789", None).unwrap();
        assert!(
            !format!("{stripe:?}").contains("sk_test"),
            "the key is never printed"
        );
    }
}
