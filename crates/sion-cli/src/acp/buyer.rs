//! The buyer's side: what an agent on Siontology does to buy from an ACP
//! merchant.
//!
//! - **On the ledger** ([`buy`]): make a checkout session, check its total
//!   and the merchant's quote against the chain the buyer pinned, sign a
//!   Siontology payment for exactly that, and complete the session with it
//!   (and AP2 mandates, if the merchant takes them).
//! - **By card** ([`buy_by_card`]): delegate the card to the merchant's PSP
//!   for this checkout only ([`delegate_card`]: an allowance of one use, the
//!   session's total, this merchant, a short time), authenticate it with 3-D
//!   Secure when the merchant asks ([`authenticate_card`]), and complete the
//!   session with the PSP's token.
//! - **Carts** ([`Buyer::cart`] and the rest): items and estimated totals
//!   before a checkout.
//!
//! Requests carry the merchant's bearer token, `API-Version`, a fresh
//! `Idempotency-Key` on every `POST`, and the buyer's `Signature` and
//! `Timestamp` when it has a [`Signer`].

use std::time::Duration;

use serde_json::{json, Value};
use sion_core::Amount;

use super::http;
use super::signing::Signer;
use super::{card, three_ds, API_VERSION, HANDLER, INSTRUMENT, MICROS_PER_CENT};
use crate::error::CliError;
use crate::x402::PaymentRequirements;

/// How long a delegated card's allowance lasts, in seconds.
pub const ALLOWANCE_FOR: u64 = 15 * 60;

/// How long [`buy`] waits for a payment the merchant is still settling.
const SETTLING_FOR: u64 = 180;

/// How long one request may take: a completion is answered once the
/// payment settles, which on a busy ledger takes a while.
const ANSWER_WITHIN: Duration = Duration::from_secs(180);

/// A buyer's side of a checkout.
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
    /// How the buyer signs its requests, if it does.
    pub signer: Option<&'a Signer>,
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

/// Sends one ACP request to `url`: the status and the JSON answered.
pub fn send(
    url: &str,
    token: &str,
    signer: Option<&Signer>,
    method: &str,
    body: Option<&Value>,
) -> Result<(u16, Value), CliError> {
    let client = http::outbound_for(true, ANSWER_WITHIN)?;
    let text = body.map(Value::to_string);
    let mut headers: Vec<(&str, String)> = vec![
        ("Authorization", format!("Bearer {token}")),
        ("API-Version", API_VERSION.to_string()),
        ("Accept", "application/json".to_string()),
        ("Request-Id", http::fresh_id("req")),
    ];
    if text.is_some() {
        headers.push(("Content-Type", "application/json".to_string()));
        headers.push(("Idempotency-Key", http::random_hex(16)));
    }
    if let Some(signer) = signer {
        let (signature, timestamp) = signer.sign(text.as_deref().unwrap_or_default(), http::now());
        headers.push(("Signature", signature));
        headers.push(("Timestamp", timestamp));
    }
    let headers: Vec<(&str, &str)> = headers.iter().map(|(n, v)| (*n, v.as_str())).collect();
    http::call(
        &client,
        method,
        url,
        &headers,
        text.as_deref().map(str::as_bytes),
    )
}

impl Buyer<'_> {
    /// Calls the merchant: `method path` with `body`.
    pub fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<(u16, Value), CliError> {
        let url = format!("{}{path}", self.base_url.trim_end_matches('/'));
        send(&url, self.token, self.signer, method, body)
    }

    fn expect(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        want: u16,
        what: &str,
    ) -> Result<Value, CliError> {
        let (status, answer) = self.call(method, path, body)?;
        if status != want {
            return Err(CliError::Protocol(format!(
                "the merchant did not {what} ({status}): {}",
                said(&answer)
            )));
        }
        Ok(answer)
    }

    /// Makes a cart of `items` (offer ids and quantities).
    pub fn cart(&self, items: &[(&str, u64)]) -> Result<Value, CliError> {
        self.expect(
            "POST",
            "/carts",
            Some(&cart_request(items)),
            201,
            "make a cart",
        )
    }

    /// Reads a cart.
    pub fn cart_get(&self, id: &str) -> Result<Value, CliError> {
        self.expect("GET", &format!("/carts/{id}"), None, 200, "read the cart")
    }

    /// Replaces a cart's items.
    pub fn cart_replace(&self, id: &str, items: &[(&str, u64)]) -> Result<Value, CliError> {
        self.expect(
            "PUT",
            &format!("/carts/{id}"),
            Some(&cart_request(items)),
            200,
            "change the cart",
        )
    }

    /// Cancels a cart.
    pub fn cart_cancel(&self, id: &str) -> Result<Value, CliError> {
        self.expect(
            "POST",
            &format!("/carts/{id}/cancel"),
            Some(&json!({})),
            200,
            "cancel the cart",
        )
    }

    /// Makes a checkout session for `items` (offer ids, one per unit).
    pub fn checkout(&self, items: &[&str]) -> Result<Value, CliError> {
        self.expect(
            "POST",
            "/checkout_sessions",
            Some(&checkout_request(items)),
            201,
            "make a session",
        )
    }

    fn cancel(&self, id: &str) {
        let _ = self.call(
            "POST",
            &format!("/checkout_sessions/{id}/cancel"),
            Some(&json!({})),
        );
    }

    /// Checks a session's total against the most this buyer pays: the total,
    /// in cents.
    fn affordable(&self, session: &Value) -> Result<u64, CliError> {
        let total = total_of(session)
            .ok_or_else(|| CliError::Protocol("the session has no total".into()))?;
        let price = u128::from(total).saturating_mul(MICROS_PER_CENT);
        if price > self.max_price.0 {
            self.cancel(session["id"].as_str().unwrap_or_default());
            return Err(CliError::Usage(format!(
                "the checkout costs {} USD, more than the {} USD allowed",
                Amount(price),
                self.max_price
            )));
        }
        Ok(total)
    }

    /// Completes a session with `body`, waiting out a payment the merchant
    /// is still settling: the session as it ended. If the answer does not
    /// come, the session is read: the payment may have gone through.
    fn complete(&self, id: &str, body: &Value) -> Result<Value, CliError> {
        let path = format!("/checkout_sessions/{id}/complete");
        let mut done = match self.expect("POST", &path, Some(body), 200, "take the completion") {
            Ok(done) => done,
            Err(CliError::Connection(why)) => {
                eprintln!("warning: no answer to the completion ({why}); reading the session");
                self.expect(
                    "GET",
                    &format!("/checkout_sessions/{id}"),
                    None,
                    200,
                    "show the session",
                )?
            }
            Err(e) => return Err(e),
        };
        let since = http::now();
        while done["status"] == "complete_in_progress" && http::now() < since + SETTLING_FOR {
            std::thread::sleep(Duration::from_secs(1));
            done = self.expect(
                "GET",
                &format!("/checkout_sessions/{id}"),
                None,
                200,
                "show the session",
            )?;
        }
        Ok(done)
    }
}

/// ACP's `CheckoutSessionCreateRequest` for `items` (offer ids, one per
/// unit).
pub fn checkout_request(items: &[&str]) -> Value {
    let line_items: Vec<Value> = items.iter().map(|id| json!({"id": id})).collect();
    json!({"currency": "usd", "line_items": line_items, "capabilities": {}})
}

/// ACP's `CheckoutSessionCompleteRequest` paying on the ledger with
/// `transaction` (hex, signed and not submitted) through `handler`.
pub fn ledger_completion(handler: &Value, transaction: &str) -> Value {
    json!({
        "payment_data": {
            "handler_id": handler["id"],
            "instrument": {
                "id": http::random_hex(8),
                "handler_id": handler["id"],
                "type": INSTRUMENT,
                // ACP's instrument requires its credential's `token`.
                "credential": {"type": "signed_transaction", "token": transaction},
            },
        },
    })
}

/// ACP's `CheckoutSessionCompleteRequest` paying by card through `handler`
/// with the PSP's `token`, and the 3-D Secure result if there is one.
pub fn card_completion(handler: &Value, token: &str, authentication: Option<&Value>) -> Value {
    let mut complete = json!({
        "payment_data": {
            "handler_id": handler["id"],
            "instrument": {
                "id": http::random_hex(8),
                "handler_id": handler["id"],
                "type": "card",
                "credential": {"type": "spt", "token": token},
            },
        },
    });
    if let Some(result) = authentication {
        complete["authentication_result"] = result.clone();
    }
    complete
}

/// ACP's `CartCreateRequest` (and `CartUpdateRequest`, a whole
/// replacement) for `items`, offer ids and quantities. ACP's `Item` has no
/// quantity (though its cart example gives one): an item is named once for
/// each unit.
pub fn cart_request(items: &[(&str, u64)]) -> Value {
    let lines: Vec<Value> = items
        .iter()
        .flat_map(|(id, quantity)| (0..*quantity).map(move |_| json!({"id": id})))
        .collect();
    json!({"line_items": lines})
}

/// A session's total, in minor units.
pub fn total_of(session: &Value) -> Option<u64> {
    session["totals"]
        .as_array()?
        .iter()
        .find(|t| t["type"] == "total")
        .and_then(|t| t["amount"].as_u64())
}

/// The handler a session declares under `name`.
pub fn handler_of<'s>(session: &'s Value, name: &str) -> Option<&'s Value> {
    session["capabilities"]["payment"]["handlers"]
        .as_array()?
        .iter()
        .find(|h| h["name"] == name)
}

fn said(answer: &Value) -> String {
    let messages: Vec<&str> = answer["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| m["content"].as_str())
        .collect();
    if messages.is_empty() {
        answer["message"]
            .as_str()
            .unwrap_or("no reason given")
            .to_string()
    } else {
        messages.join("; ")
    }
}

fn completed(done: Value) -> Result<Value, CliError> {
    if done["status"] == "completed" {
        Ok(done)
    } else {
        Err(CliError::Protocol(format!(
            "the checkout did not complete ({}): {}",
            done["status"].as_str().unwrap_or("?"),
            said(&done)
        )))
    }
}

/// Buys `items` (offer ids, one per unit) on the ledger: makes a checkout
/// session, checks its total and its payment terms, pays as `payer`, and
/// completes it with what `mandates` gives for the session (its `ap2`
/// field, if the merchant takes AP2). The completed session, or why not.
pub fn buy(
    session: &mut crate::session::Session<'_>,
    payer: &crate::x402::Payer,
    buyer: &Buyer<'_>,
    items: &[&str],
    mandates: impl FnOnce(&Value) -> Result<Option<Value>, CliError>,
) -> Result<Value, CliError> {
    let created = buyer.checkout(items)?;
    let id = created["id"].as_str().unwrap_or_default().to_string();
    let handler = handler_of(&created, HANDLER)
        .cloned()
        .ok_or_else(|| CliError::Protocol(format!("the merchant takes no {HANDLER} payment")))?;
    let requirements: PaymentRequirements =
        serde_json::from_value(handler["config"]["requirements"].clone())
            .map_err(|e| CliError::Protocol(format!("the handler's requirements: {e}")))?;
    if requirements.extra["chain"].as_str() != Some(buyer.chain.to_hex().as_str()) {
        buyer.cancel(&id);
        return Err(CliError::Protocol(
            "the merchant is paid on another chain than the one pinned".into(),
        ));
    }
    let total = buyer.affordable(&created)?;
    let price: u128 = requirements.amount.parse().unwrap_or(u128::MAX);
    if price != u128::from(total).saturating_mul(MICROS_PER_CENT) {
        buyer.cancel(&id);
        return Err(CliError::Protocol(
            "the quote is not the session's total".into(),
        ));
    }
    let signed = crate::x402::sign_payment(session, payer, &requirements)?;
    let transaction = hex::encode(sion_core::encode::encode(&signed));
    let mut complete = ledger_completion(&handler, &transaction);
    match mandates(&created) {
        Ok(Some(ap2)) => complete["ap2"] = ap2,
        Ok(None) => {}
        Err(e) => {
            session.settle_all();
            return Err(e);
        }
    }
    let answered = buyer.complete(&id, &complete);
    // Paid or not, the coins the payment was signed from are the ledger's
    // to say: a refused payment was never submitted.
    session.settle_all();
    completed(answered?)
}

/// A card, and where it is delegated.
pub struct Card<'a> {
    /// The PSP's delegate payment API (its base URL).
    pub psp_url: &'a str,
    /// The bearer token the PSP issued the agent.
    pub psp_token: &'a str,
    /// The card: ACP's `PaymentMethodCard` (`type`, `card_number_type`,
    /// `number`, `exp_month`, `exp_year`, `name`, `cvc`,
    /// `display_card_funding_type`, `metadata`, …).
    pub payment_method: &'a Value,
    /// The billing address, if one is given.
    pub billing_address: Option<&'a Value>,
}

impl std::fmt::Debug for Card<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the card.
        write!(f, "Card(delegated to {})", self.psp_url)
    }
}

/// ACP's `DelegatePaymentRequest` for `card` and `session`: an allowance
/// of one use, for the card handler's merchant, the session's total and
/// [`ALLOWANCE_FOR`] seconds.
pub fn delegation_request(card: &Card<'_>, session: &Value) -> Result<Value, CliError> {
    let handler = handler_of(session, card::HANDLER).ok_or_else(|| {
        CliError::Protocol(format!("the merchant takes no {} payment", card::HANDLER))
    })?;
    let total =
        total_of(session).ok_or_else(|| CliError::Protocol("the session has no total".into()))?;
    let mut body = json!({
        "payment_method": card.payment_method,
        "allowance": {
            "reason": "one_time",
            "max_amount": total,
            "currency": "usd",
            "checkout_session_id": session["id"],
            "merchant_id": handler["config"]["merchant_id"],
            "expires_at": http::rfc3339(http::now() + ALLOWANCE_FOR),
        },
        "risk_signals": [{"type": "card_testing", "score": 0, "action": "authorized"}],
        "metadata": {"source": "siontology"},
    });
    if let Some(address) = card.billing_address {
        body["billing_address"] = address.clone();
    }
    Ok(body)
}

/// Delegates `card` for `session` ([`delegation_request`]): the PSP's vault
/// token.
pub fn delegate_card(
    card: &Card<'_>,
    session: &Value,
    signer: Option<&Signer>,
) -> Result<String, CliError> {
    let body = delegation_request(card, session)?;
    let url = format!(
        "{}/agentic_commerce/delegate_payment",
        card.psp_url.trim_end_matches('/')
    );
    let (status, answer) = send(&url, card.psp_token, signer, "POST", Some(&body))?;
    match (status, answer["id"].as_str()) {
        (201, Some(token)) => Ok(token.to_string()),
        _ => Err(CliError::Protocol(format!(
            "the PSP did not take the card ({status}): {}",
            said(&answer)
        ))),
    }
}

/// A delegate authentication provider, and how its challenges are met.
pub struct Authenticator<'a> {
    /// The provider's API (its base URL).
    pub url: &'a str,
    /// The bearer token it issued the agent.
    pub token: &'a str,
    /// Where the issuer posts back (the 3DS Method's notification, a
    /// challenge's CRes).
    pub notification_url: &'a str,
    /// The browser the buyer is in, as ACP's `channel` describes it, if any.
    pub channel: Option<&'a Value>,
    /// Meets a challenge (ACP's `challenge` action): the buyer, shown the
    /// issuer's page, answers it.
    pub challenge: &'a dyn Fn(&Value) -> Result<(), CliError>,
}

impl std::fmt::Debug for Authenticator<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Authenticator({})", self.url)
    }
}

/// ACP's `DelegateAuthenticationCreateRequest` for `card` and `session`, as
/// the session's `authentication_metadata` asks.
pub fn authentication_request(
    provider: &Authenticator<'_>,
    card: &Card<'_>,
    session: &Value,
) -> Result<Value, CliError> {
    let metadata = &session["authentication_metadata"];
    let total =
        total_of(session).ok_or_else(|| CliError::Protocol("the session has no total".into()))?;
    let merchant = metadata["acquirer_details"]["acquirer_merchant_id"]
        .as_str()
        .or_else(|| {
            handler_of(session, card::HANDLER).and_then(|h| h["config"]["merchant_id"].as_str())
        })
        .unwrap_or_default();
    let method = card.payment_method;
    let mut body = json!({
        "merchant_id": merchant,
        "payment_method": {
            "type": "card",
            "number": method["number"],
            "exp_month": method["exp_month"],
            "exp_year": method["exp_year"],
            "name": method["name"],
        },
        // This API's currency is upper case, unlike the checkout's.
        "amount": {"value": total, "currency": "USD"},
        "challenge_notification_url": provider.notification_url,
    });
    for field in ["acquirer_details", "flow_preference"] {
        if !metadata[field].is_null() {
            body[field] = metadata[field].clone();
        }
    }
    if let Some(channel) = provider.channel {
        body["channel"] = channel.clone();
    }
    Ok(body)
}

/// Authenticates `card` for `session` with 3-D Secure, as the session's
/// `authentication_metadata` asks: the provider's session made, the issuer's
/// 3DS Method run, a challenge met if the issuer wants one. ACP's checkout
/// `authentication_result`.
pub fn authenticate_card(
    provider: &Authenticator<'_>,
    card: &Card<'_>,
    session: &Value,
    signer: Option<&Signer>,
) -> Result<Value, CliError> {
    let body = authentication_request(provider, card, session)?;
    let base = provider.url.trim_end_matches('/');
    let ask = |method: &str, path: &str, body: Option<&Value>| {
        send(
            &format!("{base}{path}"),
            provider.token,
            signer,
            method,
            body,
        )
    };
    let (status, created) = ask("POST", "/delegate_authentication", Some(&body))?;
    if status != 201 {
        return Err(CliError::Protocol(format!(
            "the authentication provider refused ({status}): {}",
            said(&created)
        )));
    }
    let id = created["authentication_session_id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    if created["status"] == "not_supported" {
        return Ok(three_ds::checkout_result(&created));
    }
    // The 3DS Method: the issuer fingerprints the browser, then the
    // provider is told whether it ran.
    let completion = match created["action"]["type"].as_str() {
        Some("fingerprint") => {
            let fingerprint = &created["action"]["fingerprint"];
            let data = three_ds::method_data(
                fingerprint["three_ds_server_trans_id"]
                    .as_str()
                    .unwrap_or_default(),
                provider.notification_url,
            );
            let url = fingerprint["three_ds_method_url"]
                .as_str()
                .unwrap_or_default();
            let ran =
                post_form(url, &[("threeDSMethodData", &data)]).is_ok_and(|status| status == 200);
            if ran {
                "Y"
            } else {
                "N"
            }
        }
        _ => "U",
    };
    let mut asked = json!({"fingerprint_completion": completion});
    if let Some(channel) = provider.channel {
        asked["channel"] = channel.clone();
    }
    let (status, authenticated) = ask(
        "POST",
        &format!("/delegate_authentication/{id}/authenticate"),
        Some(&asked),
    )?;
    if status != 200 {
        return Err(CliError::Protocol(format!(
            "the authentication did not go on ({status}): {}",
            said(&authenticated)
        )));
    }
    if authenticated["status"] == "action_required"
        && authenticated["action"]["type"] == "challenge"
    {
        (provider.challenge)(&authenticated["action"]["challenge"])?;
    }
    let (status, retrieved) = ask("GET", &format!("/delegate_authentication/{id}"), None)?;
    if status != 200 {
        return Err(CliError::Protocol(format!(
            "the authentication could not be read ({status})"
        )));
    }
    Ok(three_ds::checkout_result(&retrieved))
}

/// Posts a form to an issuer's page: the status.
fn post_form(url: &str, fields: &[(&str, &str)]) -> Result<u16, CliError> {
    let client = http::outbound(true)?;
    let fields: Vec<(String, String)> = fields
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let body = card::form(&fields);
    let response = client
        .send(
            "POST",
            url,
            &[("Content-Type", "application/x-www-form-urlencoded")],
            Some(body.as_bytes()),
        )
        .map_err(|e| CliError::Connection(e.to_string()))?;
    Ok(response.status)
}

/// Meets the simulated issuer's challenge ([`three_ds::SimulatedAcs`]) with
/// `code`: opens its page with the CReq and answers it. Only the simulated
/// issuer's page is known; a real issuer's is the buyer's to answer.
pub fn answer_simulated_challenge(challenge: &Value, code: &str) -> Result<(), CliError> {
    let acs = challenge["acs_url"].as_str().unwrap_or_default();
    let status = post_form(acs, &[("creq", &three_ds::creq(challenge))])?;
    if status != 200 {
        return Err(CliError::Protocol(format!(
            "the issuer's challenge did not open ({status})"
        )));
    }
    let acs_trans_id = challenge["acs_trans_id"].as_str().unwrap_or_default();
    let status = post_form(
        &format!("{acs}/submit"),
        &[("acsTransID", acs_trans_id), ("otp", code)],
    )?;
    if status != 200 {
        return Err(CliError::Protocol(format!(
            "the issuer did not take the answer ({status})"
        )));
    }
    Ok(())
}

/// Buys `items` by card: makes a checkout session, checks its total,
/// delegates `card` to the merchant's PSP for it, authenticates the card
/// with `authenticator` when the merchant or the issuer asks for 3-D
/// Secure, and completes the session with the PSP's token.
pub fn buy_by_card(
    buyer: &Buyer<'_>,
    items: &[&str],
    card_given: &Card<'_>,
    authenticator: Option<&Authenticator<'_>>,
) -> Result<Value, CliError> {
    let created = buyer.checkout(items)?;
    let id = created["id"].as_str().unwrap_or_default().to_string();
    let Some(handler) = handler_of(&created, card::HANDLER).cloned() else {
        buyer.cancel(&id);
        return Err(CliError::Protocol(format!(
            "the merchant takes no {} payment",
            card::HANDLER
        )));
    };
    buyer.affordable(&created)?;
    let authenticate = |session: &Value| -> Result<Value, CliError> {
        let provider = authenticator.ok_or_else(|| {
            CliError::Usage(
                "the merchant asks for 3-D Secure: name an authentication provider".into(),
            )
        })?;
        authenticate_card(provider, card_given, session, buyer.signer)
    };
    let mut authentication = if created["authentication_metadata"].is_null() {
        None
    } else {
        Some(authenticate(&created)?)
    };
    let token = delegate_card(card_given, &created, buyer.signer)?;
    let mut done = Value::Null;
    for _ in 0..2 {
        let complete = card_completion(&handler, &token, authentication.as_ref());
        done = buyer.complete(&id, &complete)?;
        // The issuer asked for 3-D Secure the merchant did not: once.
        if done["status"] == "authentication_required" && authentication.is_none() {
            authentication = Some(authenticate(&done)?);
            continue;
        }
        break;
    }
    completed(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_is_read_for_its_total_and_handlers() {
        let session = json!({
            "totals": [{"type": "subtotal", "amount": 450}, {"type": "total", "amount": 450}],
            "capabilities": {"payment": {"handlers": [{"name": HANDLER, "id": "sion_ledger"}]}},
        });
        assert_eq!(total_of(&session), Some(450));
        assert_eq!(handler_of(&session, HANDLER).unwrap()["id"], "sion_ledger");
        assert!(handler_of(&session, card::HANDLER).is_none());
        assert_eq!(total_of(&json!({})), None);
    }

    #[test]
    fn messages_explain_a_refusal() {
        assert_eq!(
            said(&json!({"messages": [{"content": "a"}, {"content": "b"}]})),
            "a; b"
        );
        assert_eq!(said(&json!({"message": "no"})), "no");
        assert!(completed(json!({"status": "ready_for_payment"})).is_err());
        assert!(completed(json!({"status": "completed"})).is_ok());
    }
}
