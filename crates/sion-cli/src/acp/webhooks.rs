//! ACP's order webhooks: the merchant tells the agent's platform about an
//! order when it is made (`order_create`) and whenever it changes
//! (`order_update`), each event carrying the whole order, never a delta.
//!
//! Each `POST` is signed as ACP's webhook API has it: `Merchant-Signature:
//! t=<unix seconds>,v1=<hex HMAC-SHA256(timestamp + "." + raw body,
//! secret)>`, the receiver refusing it (`401 invalid_signature`) if it is
//! missing, malformed, more than 300 seconds old or does not verify.
//!
//! Events wait in an outbox the merchant keeps with its state, and are
//! delivered in order, retried with backoff (5 seconds, doubling, at most
//! an hour) until the receiver answers `2xx`, and given up after
//! [`MAX_ATTEMPTS`]: an event the receiver never takes stays in the outbox,
//! marked, for its operator to see.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::http::{self, Request, Response};
use crate::error::CliError;

/// The header a webhook is signed in.
pub const HEADER: &str = "Merchant-Signature";

/// How old a webhook's timestamp may be, in seconds.
pub const TOLERANCE: u64 = 300;

/// How many times an event is tried before it is given up.
pub const MAX_ATTEMPTS: u32 = 12;

fn hmac(secret: &[u8], data: &[u8]) -> String {
    let key = aws_lc_rs::hmac::Key::new(aws_lc_rs::hmac::HMAC_SHA256, secret);
    hex::encode(aws_lc_rs::hmac::sign(&key, data).as_ref())
}

/// The `Merchant-Signature` of `body` at `timestamp`.
pub fn signature(secret: &[u8], timestamp: u64, body: &str) -> String {
    format!(
        "t={timestamp},v1={}",
        hmac(secret, format!("{timestamp}.{body}").as_bytes())
    )
}

/// Checks a `Merchant-Signature` over `body` at `now`: its timestamp, if it
/// verifies.
pub fn verify(header: &str, body: &str, secret: &[u8], now: u64) -> Result<u64, String> {
    let mut timestamp = None;
    let mut signatures = Vec::new();
    for part in header.split(',') {
        match part.trim().split_once('=') {
            Some(("t", t)) if t.bytes().all(|b| b.is_ascii_digit()) && !t.is_empty() => {
                timestamp = t.parse::<u64>().ok();
            }
            Some(("v1", v)) if v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit()) => {
                signatures.push(v.to_ascii_lowercase());
            }
            _ => {}
        }
    }
    let Some(timestamp) = timestamp.filter(|_| !signatures.is_empty()) else {
        return Err("Merchant-Signature must be t=<timestamp>,v1=<64_hex>.".into());
    };
    if timestamp.abs_diff(now) > TOLERANCE {
        return Err("Timestamp outside allowed window.".into());
    }
    let expected = hmac(secret, format!("{timestamp}.{body}").as_bytes());
    if signatures
        .iter()
        .any(|given| http::same_secret(given, &expected))
    {
        Ok(timestamp)
    } else {
        Err("Webhook signature verification failed.".into())
    }
}

/// An event: `order_create` or `order_update`, with the whole order.
pub fn event(kind: &str, order: &Value) -> Value {
    json!({"type": kind, "data": order})
}

/// An event waiting to be delivered, or delivered.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Delivery {
    /// Its id, sent as `Request-Id`.
    pub id: String,
    /// The event's JSON, as it is signed and sent.
    pub body: String,
    /// How many times it was tried.
    pub attempts: u32,
    /// When it is next tried, in seconds since 1970.
    pub next_at: u64,
    /// When it was delivered.
    #[serde(default)]
    pub delivered_at: Option<u64>,
    /// Why the last try failed.
    #[serde(default)]
    pub last_error: Option<String>,
    /// Whether it was given up.
    #[serde(default)]
    pub gave_up: bool,
}

impl Delivery {
    /// `event`, to be delivered from `now`.
    pub fn new(event: &Value, now: u64) -> Delivery {
        Delivery {
            id: http::fresh_id("evt"),
            body: event.to_string(),
            attempts: 0,
            next_at: now,
            delivered_at: None,
            last_error: None,
            gave_up: false,
        }
    }

    /// Whether it should be tried at `now`.
    pub fn due(&self, now: u64) -> bool {
        self.delivered_at.is_none() && !self.gave_up && self.next_at <= now
    }

    /// Notes a failed try at `now`: the next one later, or none.
    pub fn failed(&mut self, now: u64, why: String) {
        self.attempts += 1;
        self.last_error = Some(why);
        if self.attempts >= MAX_ATTEMPTS {
            self.gave_up = true;
        } else {
            let wait = 5u64
                .saturating_mul(1 << (self.attempts - 1).min(20))
                .min(3600);
            self.next_at = now + wait;
        }
    }
}

/// Where a merchant's webhooks go, and the secret they are signed with.
#[derive(Clone)]
pub struct Hook {
    /// The receiver's URL.
    pub url: String,
    secret: Vec<u8>,
}

impl std::fmt::Debug for Hook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The secret is never printed.
        write!(f, "Hook({})", self.url)
    }
}

impl Hook {
    /// Webhooks to `url`, signed with `secret` (16 bytes or more).
    pub fn new(url: &str, secret: &[u8]) -> Result<Hook, CliError> {
        if secret.len() < 16 {
            return Err(CliError::Usage(
                "a webhook secret has at least 16 bytes".into(),
            ));
        }
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            return Err(CliError::Usage(format!("{url} is not an HTTP URL")));
        }
        Ok(Hook {
            url: url.to_string(),
            secret: secret.to_vec(),
        })
    }

    /// Sends `delivery` once, at `now`.
    pub fn send(
        &self,
        client: &sion_http::Client,
        delivery: &Delivery,
        now: u64,
    ) -> Result<(), String> {
        let signed = signature(&self.secret, now, &delivery.body);
        let timestamp = http::rfc3339(now);
        let headers = [
            ("Content-Type", "application/json"),
            (HEADER, signed.as_str()),
            ("Request-Id", delivery.id.as_str()),
            ("Timestamp", timestamp.as_str()),
        ];
        match http::call(
            client,
            "POST",
            &self.url,
            &headers,
            Some(delivery.body.as_bytes()),
        ) {
            Ok((status, _)) if (200..300).contains(&status) => Ok(()),
            Ok((status, body)) => Err(format!(
                "the receiver answered {status}: {}",
                body["message"]
            )),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// A receiver's handling of a webhook: the event, if it is signed with
/// `secret`; else the `401` ACP's webhook API answers.
pub fn receive(request: &Request<'_>, secret: &[u8], now: u64) -> Result<Value, Response> {
    let refuse =
        |message: &str| http::error(401, "invalid_request", "invalid_signature", message, None);
    let header = request
        .header(HEADER)
        .ok_or_else(|| refuse("Missing Merchant-Signature header."))?;
    verify(header, request.body, secret, now).map_err(|why| refuse(&why))?;
    let event: Value = serde_json::from_str(request.body)
        .map_err(|e| http::invalid("invalid", &format!("the event is not JSON: {e}"), None))?;
    if !matches!(
        event["type"].as_str(),
        Some("order_create" | "order_update")
    ) || event["data"]["type"] != "order"
    {
        return Err(http::invalid(
            "invalid",
            "an event is an order_create or order_update with the whole order",
            None,
        ));
    }
    Ok(event)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &[u8] = b"whsec_0123456789abcdef";

    #[test]
    fn a_signature_verifies_only_for_its_body_secret_and_time() {
        let body = r#"{"type":"order_create","data":{"type":"order","id":"ord_1"}}"#;
        let header = signature(SECRET, 1_800_000_000, body);
        assert!(header.starts_with("t=1800000000,v1="));
        assert_eq!(
            verify(&header, body, SECRET, 1_800_000_100),
            Ok(1_800_000_000)
        );
        assert!(verify(&header, body, SECRET, 1_800_000_000 + TOLERANCE + 1).is_err());
        assert!(verify(
            &header,
            &body.replace("ord_1", "ord_2"),
            SECRET,
            1_800_000_000
        )
        .is_err());
        assert!(verify(&header, body, b"another secret, long enough", 1_800_000_000).is_err());
        assert!(verify("v1=abc", body, SECRET, 1_800_000_000).is_err());
    }

    #[test]
    fn the_signature_is_the_hex_hmac_of_timestamp_dot_body() {
        // Known answer, from OpenSSL (`openssl dgst -sha256 -hmac`): the
        // scheme Stripe-style verifiers (acp-node's `Webhooks`, for one)
        // compute, `t=<unix>,v1=<hex HMAC-SHA256(t + "." + body)>`.
        let body = r#"{"type":"order_create","data":{"type":"order","id":"ord_1"}}"#;
        assert_eq!(
            signature(SECRET, 1_800_000_000, body),
            "t=1800000000,v1=349a502f0aefec793bdbb28f31c1537fdf66d1bfac0330ae6c0eeb9387c57560"
        );
    }

    #[test]
    fn a_failed_delivery_backs_off_and_is_given_up() {
        let mut delivery = Delivery::new(&event("order_create", &json!({"type": "order"})), 100);
        assert!(delivery.due(100));
        delivery.failed(100, "down".into());
        assert_eq!(delivery.next_at, 105);
        delivery.failed(105, "down".into());
        assert_eq!(delivery.next_at, 115);
        for _ in 2..MAX_ATTEMPTS {
            delivery.failed(200, "down".into());
        }
        assert!(delivery.gave_up);
        assert!(!delivery.due(u64::MAX));
    }

    #[test]
    fn a_receiver_takes_only_signed_order_events() {
        let body = r#"{"type":"order_update","data":{"type":"order","id":"ord_1"}}"#;
        let headers = vec![(HEADER.to_string(), signature(SECRET, 1_800_000_000, body))];
        let request = Request {
            method: "POST",
            path: "/agentic_checkout/webhooks/order_events",
            headers: &headers,
            body,
        };
        assert_eq!(
            receive(&request, SECRET, 1_800_000_001).unwrap()["data"]["id"],
            "ord_1"
        );
        let unsigned = Request {
            headers: &[],
            ..request.clone()
        };
        assert_eq!(
            receive(&unsigned, SECRET, 1_800_000_001)
                .unwrap_err()
                .status,
            401
        );
    }
}
