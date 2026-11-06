//! What every ACP service here shares: requests and answers, ACP's flat
//! errors, the gate each request passes (a bearer token, `API-Version`,
//! and the client's signature when it signs), idempotency keys, and
//! serving over HTTP.
//!
//! Idempotency is ACP's (SEP #120, the delegate payment RFC §5, which the
//! checkout and cart RFCs follow): a key is scoped to the client and the
//! path; the same key with the same body is answered with the stored
//! answer and `Idempotent-Replayed: true`, never run again; with another
//! body it is refused (`422 idempotency_conflict`, or `409` where a service
//! says so); while the first is still running it is `409
//! idempotency_in_flight` with `Retry-After`; a `5xx` answer is not kept;
//! keys are kept 24 hours. Bodies are compared as JSON values: by their RFC
//! 8785 canonical form, so key order and `1.0` against `1` do not matter
//! and `null` against absent does.

use std::collections::BTreeMap;
use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::signing::{self, Verifier};
use crate::error::CliError;

/// An HTTP request to a service.
#[derive(Clone, Debug)]
pub struct Request<'a> {
    /// `GET`, `POST`, `PUT` or `PATCH`.
    pub method: &'a str,
    /// The path, without the query.
    pub path: &'a str,
    /// Its headers.
    pub headers: &'a [(String, String)],
    /// Its body, as sent.
    pub body: &'a str,
}

impl Request<'_> {
    /// A header's value, its name read without case.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// A service's answer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Response {
    /// Its status.
    pub status: u16,
    /// Headers beyond its `Content-Type`.
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    /// Its body, as JSON.
    pub body: Value,
    /// An HTML page instead of JSON (the simulated ACS's pages).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<String>,
}

impl Response {
    /// A JSON answer.
    pub fn json(status: u16, body: Value) -> Response {
        Response {
            status,
            headers: Vec::new(),
            body,
            page: None,
        }
    }

    /// An HTML page.
    pub fn page(status: u16, html: String) -> Response {
        Response {
            status,
            headers: Vec::new(),
            body: Value::Null,
            page: Some(html),
        }
    }

    /// It, with one more header.
    pub fn with_header(mut self, name: &str, value: &str) -> Response {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    /// The value of one of its headers.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// An error in ACP's flat shape: `{type, code, message, param?}`.
pub fn error(status: u16, kind: &str, code: &str, message: &str, param: Option<&str>) -> Response {
    let mut body = json!({"type": kind, "code": code, "message": message});
    if let Some(param) = param {
        body["param"] = json!(param);
    }
    Response::json(status, body)
}

/// A `400 invalid_request` with `code`.
pub fn invalid(code: &str, message: &str, param: Option<&str>) -> Response {
    error(400, "invalid_request", code, message, param)
}

/// A `404 invalid_request not_found`.
pub fn not_found(message: &str) -> Response {
    error(404, "invalid_request", "not_found", message, None)
}

/// Now, in seconds since 1970.
pub fn now() -> u64 {
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

/// Seconds since 1970 of an RFC 3339 date-time, or of a whole number of
/// seconds.
pub fn seconds_of(text: &str) -> Option<u64> {
    let text = text.trim();
    if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) {
        return text.parse().ok();
    }
    crate::ap2::seconds_of(text).and_then(|s| u64::try_from(s).ok())
}

/// `bytes` random bytes, in hex.
pub fn random_hex(bytes: usize) -> String {
    use aws_lc_rs::rand::SecureRandom;
    let mut buffer = vec![0u8; bytes];
    aws_lc_rs::rand::SystemRandom::new()
        .fill(&mut buffer)
        .expect("the system has randomness");
    hex::encode(buffer)
}

/// A new identifier: `prefix`, then 96 random bits.
pub fn fresh_id(prefix: &str) -> String {
    format!("{prefix}_{}", random_hex(12))
}

/// Whether two secrets are the same, in time that does not depend on where
/// they differ.
pub fn same_secret(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

// ----- The gate -----------------------------------------------------------

/// A client a service knows by its bearer token, and how it signs.
#[derive(Clone)]
pub struct Client {
    /// What it is called here.
    pub name: String,
    token: String,
    /// How its `Signature` header is checked, if it signs.
    pub verifier: Option<Verifier>,
    /// Whether every request of its must be signed.
    pub must_sign: bool,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The token is a secret: never printed.
        write!(
            f,
            "Client({}, signs: {})",
            self.name,
            self.verifier.is_some()
        )
    }
}

impl Client {
    /// A client known by `token` (16 characters or more).
    pub fn new(name: &str, token: &str) -> Result<Client, CliError> {
        if token.len() < 16 {
            return Err(CliError::Usage(format!(
                "{name}'s bearer token has at least 16 characters"
            )));
        }
        Ok(Client {
            name: name.to_string(),
            token: token.to_string(),
            verifier: None,
            must_sign: false,
        })
    }

    /// It, its requests checked by `verifier`; `must`, an unsigned request
    /// of its is refused.
    pub fn signing(mut self, verifier: Verifier, must: bool) -> Client {
        self.verifier = Some(verifier);
        self.must_sign = must;
        self
    }
}

/// Reads clients from `name=token` or bare `token` entries, comma-separated
/// (an environment variable's value): a bare token is called `client-N`.
pub fn clients_of(list: &str) -> Result<Vec<Client>, CliError> {
    list.split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .enumerate()
        .map(|(index, entry)| match entry.split_once('=') {
            Some((name, token)) => Client::new(name.trim(), token.trim()),
            None => Client::new(&format!("client-{index}"), entry),
        })
        .collect()
}

/// The checks every authenticated request passes.
#[derive(Clone, Debug)]
pub struct Gate {
    clients: Vec<Client>,
    versions: Vec<String>,
}

impl Gate {
    /// A gate for `clients`, taking `versions` of the API.
    pub fn new(clients: Vec<Client>, versions: &[&str]) -> Result<Gate, CliError> {
        if clients.is_empty() {
            return Err(CliError::Usage("a service has at least one client".into()));
        }
        Ok(Gate {
            clients,
            versions: versions.iter().map(|v| v.to_string()).collect(),
        })
    }

    /// The versions it takes.
    pub fn versions(&self) -> &[String] {
        &self.versions
    }

    /// The client `request` comes from, if it passes: a bearer token one of
    /// them holds, an `API-Version` taken here, and the client's signature
    /// over the body if it signs.
    pub fn admit(&self, request: &Request<'_>) -> Result<&Client, Response> {
        let presented = request
            .header("Authorization")
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(str::trim);
        let client = presented
            .and_then(|p| self.clients.iter().find(|c| same_secret(&c.token, p)))
            .ok_or_else(|| {
                error(
                    401,
                    "invalid_request",
                    "unauthorized",
                    "a bearer token this service issued is needed",
                    None,
                )
            })?;
        match request.header("API-Version") {
            None => {
                let mut refused = invalid("missing_api_version", "API-Version is required", None);
                refused.body["supported_versions"] = json!(self.versions);
                return Err(refused);
            }
            Some(version) if !self.versions.iter().any(|v| v == version) => {
                let mut refused = invalid(
                    "unsupported_api_version",
                    &format!("API-Version {version} is not served"),
                    None,
                );
                refused.body["supported_versions"] = json!(self.versions);
                return Err(refused);
            }
            Some(_) => {}
        }
        if let Some(verifier) = &client.verifier {
            let unsigned = || {
                error(
                    401,
                    "invalid_request",
                    "invalid_signature",
                    "this client signs its requests: Signature and Timestamp are required",
                    None,
                )
            };
            match (request.header("Signature"), request.header("Timestamp")) {
                (Some(signature), Some(timestamp)) => {
                    verifier
                        .check(signature, timestamp, request.body, now())
                        .map_err(|why| {
                            error(401, "invalid_request", "invalid_signature", &why, None)
                        })?;
                }
                _ if client.must_sign => return Err(unsigned()),
                _ => {}
            }
        }
        Ok(client)
    }
}

/// The body of `request` as a JSON object (an empty body is `{}`).
pub fn body_of(request: &Request<'_>) -> Result<Value, Response> {
    let body: Value = if request.body.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(request.body)
            .map_err(|e| invalid("invalid", &format!("the body is not JSON: {e}"), None))?
    };
    if body.is_object() {
        Ok(body)
    } else {
        Err(invalid("invalid", "the body is a JSON object", None))
    }
}

// ----- Idempotency --------------------------------------------------------

/// How long an idempotency key's answer is kept, in seconds.
pub const KEYS_KEPT: u64 = 24 * 3600;

/// What a key holds: the body it was first used with, and the answer, once
/// there is one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Kept {
    /// The SHA-256 of the body's canonical JSON.
    pub fingerprint: String,
    /// The answer; none while the first request is still running.
    pub response: Option<Response>,
    /// When it was first used, in seconds since 1970.
    pub at: u64,
}

/// The keys a service keeps, by client, path and key.
pub type Keys = BTreeMap<String, Kept>;

/// A key claimed for a request that is now running.
#[derive(Clone, Debug)]
pub struct Slot {
    id: String,
}

/// The canonical fingerprint of a body.
pub fn fingerprint(body: &Value) -> String {
    hex::encode(aws_lc_rs::digest::digest(
        &aws_lc_rs::digest::SHA256,
        signing::canonical(body).as_bytes(),
    ))
}

/// Claims `key` for `client`'s request to `path` with `body`; or the
/// answer it already has, a conflict (with `conflict` as its status), or
/// that the first is still running.
pub fn claim(
    keys: &mut Keys,
    conflict: u16,
    client: &str,
    path: &str,
    key: &str,
    body: &Value,
) -> Result<Slot, Response> {
    let at = now();
    keys.retain(|_, kept| kept.response.is_none() || at.saturating_sub(kept.at) < KEYS_KEPT);
    let id = format!("{client}\u{0}{path}\u{0}{key}");
    let print = fingerprint(body);
    match keys.get(&id) {
        Some(kept) if kept.fingerprint != print => Err(error(
            conflict,
            "invalid_request",
            "idempotency_conflict",
            "Idempotency-Key has already been used with a different request body",
            None,
        )),
        // Answered before: the stored answer, not a second run.
        Some(Kept {
            response: Some(response),
            ..
        }) => Err(response.clone().with_header("Idempotent-Replayed", "true")),
        Some(_) => Err(error(
            409,
            "invalid_request",
            "idempotency_in_flight",
            "A request with this Idempotency-Key is currently being processed",
            None,
        )
        .with_header("Retry-After", "1")),
        None => {
            keys.insert(
                id.clone(),
                Kept {
                    fingerprint: print,
                    response: None,
                    at,
                },
            );
            Ok(Slot { id })
        }
    }
}

/// Keeps `response` for the key `slot` claimed (a `5xx` is not kept: the
/// key is free again).
pub fn settle(keys: &mut Keys, slot: Slot, response: &Response) {
    if response.status >= 500 {
        keys.remove(&slot.id);
    } else if let Some(kept) = keys.get_mut(&slot.id) {
        kept.response = Some(response.clone());
    }
}

/// Keys left running when a service stopped are let go: their requests
/// never answered.
pub fn release_running(keys: &mut Keys) {
    keys.retain(|_, kept| kept.response.is_some());
}

/// The `Idempotency-Key` of a request, if it has a valid one; a refusal if
/// it is malformed, or missing where `required`.
pub fn key_of<'r>(request: &'r Request<'_>, required: bool) -> Result<Option<&'r str>, Response> {
    match request.header("Idempotency-Key") {
        None if required => Err(invalid(
            "idempotency_key_required",
            "Idempotency-Key header is required on all POST requests",
            None,
        )),
        None => Ok(None),
        Some(key) if key.is_empty() || key.len() > 255 => Err(invalid(
            "invalid",
            "an Idempotency-Key has 1 to 255 characters",
            None,
        )),
        Some(key) => Ok(Some(key)),
    }
}

/// Echoes the correlation headers ACP asks a service to echo.
pub fn echo(request: &Request<'_>, mut response: Response) -> Response {
    if let Some(id) = request.header("Request-Id") {
        response.headers.push(("Request-Id".into(), id.to_string()));
    }
    if matches!(request.method, "POST" | "PUT" | "PATCH") {
        if let Some(key) = request.header("Idempotency-Key") {
            response
                .headers
                .push(("Idempotency-Key".into(), key.to_string()));
        }
    }
    response
}

// ----- Serving ------------------------------------------------------------

/// A service answering ACP's requests.
pub trait Service: Sync {
    /// Its answer to `request`.
    fn answer(&self, request: &Request<'_>) -> Response;

    /// Work done between requests, about once a second (delivering
    /// webhooks, finishing interrupted payments).
    fn tick(&self) {}
}

/// The longest request body taken, in bytes.
pub const MAX_BODY: u64 = 1 << 20;

/// The most requests answered at once.
pub const AT_ONCE: usize = 64;

/// Serves `service` on `server` until the server is unblocked (or for
/// `requests` requests), several at once, ticking it about once a second.
pub fn serve<S: Service + ?Sized>(
    service: &S,
    server: &tiny_http::Server,
    requests: Option<usize>,
) {
    let at_once = AtomicUsize::new(0);
    let stopped = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let ticker = scope.spawn(|| {
            while !stopped.load(Ordering::SeqCst) {
                service.tick();
                std::thread::park_timeout(Duration::from_secs(1));
            }
        });
        for (index, mut request) in server.incoming_requests().enumerate() {
            while at_once.load(Ordering::SeqCst) >= AT_ONCE {
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
                        "the body is too long, or not UTF-8",
                        None,
                    )
                } else {
                    service.answer(&Request {
                        method: &method,
                        path: &path,
                        headers: &headers,
                        body: &body,
                    })
                };
                let (text, kind) = match &response.page {
                    Some(html) => (html.clone(), &b"text/html; charset=utf-8"[..]),
                    None => (response.body.to_string(), &b"application/json"[..]),
                };
                let mut reply = tiny_http::Response::from_string(text)
                    .with_status_code(response.status)
                    .with_header(
                        tiny_http::Header::from_bytes(&b"Content-Type"[..], kind)
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
        stopped.store(true, Ordering::SeqCst);
        ticker.thread().unpark();
    });
}

/// The client every outbound ACP call is made with: bounded, with TLS,
/// reaching local addresses only if `local` (a service an operator names,
/// or a test's loopback).
pub fn outbound(local: bool) -> Result<sion_http::Client, CliError> {
    outbound_for(local, Duration::from_secs(60))
}

/// [`outbound`], waiting up to `timeout` for a whole answer.
pub fn outbound_for(local: bool, timeout: Duration) -> Result<sion_http::Client, CliError> {
    sion_http::Client::new(sion_http::Options {
        allow_private: local,
        timeout,
        ..sion_http::Options::default()
    })
    .map_err(|e| CliError::Connection(e.to_string()))
}

/// Sends `body` to `url` with `headers`: the status and the JSON answered
/// (an answer that is not JSON is `{"message": <its text>}`).
pub fn call(
    client: &sion_http::Client,
    method: &str,
    url: &str,
    headers: &[(&str, &str)],
    body: Option<&[u8]>,
) -> Result<(u16, Value), CliError> {
    let response = client
        .send(method, url, headers, body)
        .map_err(|e| CliError::Connection(e.to_string()))?;
    let status = response.status;
    let bytes = response
        .read_all(4 << 20)
        .map_err(|e| CliError::Connection(e.to_string()))?;
    let text = String::from_utf8_lossy(&bytes);
    let value = serde_json::from_str(&text).unwrap_or_else(|_| json!({"message": text}));
    Ok((status, value))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request<'a>(headers: &'a [(String, String)], body: &'a str) -> Request<'a> {
        Request {
            method: "POST",
            path: "/x",
            headers,
            body,
        }
    }

    #[test]
    fn a_key_replays_its_answer_and_refuses_another_body() {
        let mut keys = Keys::new();
        let body = json!({"a": 1, "b": [1, 2]});
        let slot = claim(&mut keys, 422, "c", "/p", "k", &body).unwrap();
        // Still running: in flight.
        let busy = claim(&mut keys, 422, "c", "/p", "k", &body).unwrap_err();
        assert_eq!(
            (busy.status, busy.body["code"].as_str()),
            (409, Some("idempotency_in_flight"))
        );
        assert_eq!(busy.header("Retry-After"), Some("1"));
        settle(&mut keys, slot, &Response::json(201, json!({"id": "x"})));
        // The same body, its keys reordered and a number written 1.0.
        let same: Value = serde_json::from_str(r#"{"b": [1, 2], "a": 1.0}"#).unwrap();
        let replay = claim(&mut keys, 422, "c", "/p", "k", &same).unwrap_err();
        assert_eq!(replay.status, 201);
        assert_eq!(replay.header("Idempotent-Replayed"), Some("true"));
        // Another body: a conflict, with the status the service names.
        let other = claim(&mut keys, 409, "c", "/p", "k", &json!({"a": 2})).unwrap_err();
        assert_eq!(
            (other.status, other.body["code"].as_str()),
            (409, Some("idempotency_conflict"))
        );
        // null is not absent.
        let nulled = claim(
            &mut keys,
            422,
            "c",
            "/p",
            "k",
            &json!({"a": 1, "b": [1, 2], "c": null}),
        );
        assert_eq!(nulled.unwrap_err().status, 422);
        // Scoped to client and path.
        assert!(claim(&mut keys, 422, "d", "/p", "k", &body).is_ok());
        assert!(claim(&mut keys, 422, "c", "/q", "k", &body).is_ok());
    }

    #[test]
    fn a_server_error_frees_the_key() {
        let mut keys = Keys::new();
        let slot = claim(&mut keys, 422, "c", "/p", "k", &json!({})).unwrap();
        settle(&mut keys, slot, &Response::json(503, json!({})));
        assert!(claim(&mut keys, 422, "c", "/p", "k", &json!({})).is_ok());
    }

    #[test]
    fn the_gate_takes_a_token_a_version_and_a_signature() {
        let signer = signing::Signer::hmac(b"0123456789abcdef0123456789abcdef");
        let gate = Gate::new(
            vec![
                Client::new("agent", "agent-token-0123456789").unwrap(),
                Client::new("signer", "signer-token-0123456789")
                    .unwrap()
                    .signing(signer.verifier(), true),
            ],
            &["2026-04-17"],
        )
        .unwrap();
        let with = |token: &str, version: Option<&str>| {
            let mut headers = vec![("Authorization".to_string(), format!("Bearer {token}"))];
            if let Some(version) = version {
                headers.push(("API-Version".to_string(), version.to_string()));
            }
            headers
        };
        let headers = with("agent-token-0123456789", Some("2026-04-17"));
        assert_eq!(gate.admit(&request(&headers, "{}")).unwrap().name, "agent");
        let headers = with("wrong-token-0123456789", Some("2026-04-17"));
        assert_eq!(
            gate.admit(&request(&headers, "{}")).unwrap_err().status,
            401
        );
        let headers = with("agent-token-0123456789", Some("2025-01-01"));
        let refused = gate.admit(&request(&headers, "{}")).unwrap_err();
        assert_eq!(refused.body["code"], "unsupported_api_version");
        assert_eq!(refused.body["supported_versions"], json!(["2026-04-17"]));
        // A client that signs: unsigned is refused, signed is let in.
        let headers = with("signer-token-0123456789", Some("2026-04-17"));
        assert_eq!(
            gate.admit(&request(&headers, "{}")).unwrap_err().body["code"],
            "invalid_signature"
        );
        let (signature, timestamp) = signer.sign("{\"a\":1}", now());
        let mut signed = headers.clone();
        signed.push(("Signature".into(), signature));
        signed.push(("Timestamp".into(), timestamp));
        assert!(gate.admit(&request(&signed, "{\"a\":1}")).is_ok());
        // Another body under the same signature is not.
        assert!(gate.admit(&request(&signed, "{\"a\":2}")).is_err());
    }

    #[test]
    fn clients_are_read_as_names_and_tokens() {
        let clients = clients_of("openai=token-aaaaaaaaaaaaaaaa, token-bbbbbbbbbbbbbbbb").unwrap();
        assert_eq!(clients[0].name, "openai");
        assert_eq!(clients[1].name, "client-1");
        assert!(clients_of("short").is_err());
        assert!(
            !format!("{:?}", clients[0]).contains("aaaa"),
            "a token is never printed"
        );
    }

    #[test]
    fn times_are_written_and_read_as_rfc_3339_or_seconds() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(1_777_593_599), "2026-04-30T23:59:59Z");
        assert_eq!(seconds_of("2026-04-30T23:59:59Z"), Some(1_777_593_599));
        assert_eq!(seconds_of("1777593599"), Some(1_777_593_599));
        assert_eq!(seconds_of("yesterday"), None);
    }
}
