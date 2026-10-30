//! MCP servers reached over HTTP.
//!
//! - **Streamable HTTP, 2026-07-28.** Every request is its own `POST` to
//!   the server's one endpoint, its version, client and capabilities in its
//!   `_meta` and mirrored in headers (`MCP-Protocol-Version`, `Mcp-Method`,
//!   `Mcp-Name`, and `Mcp-Param-*` for the arguments a tool's schema marks
//!   with `x-mcp-header`). The answer is a JSON object, or a stream of
//!   Server-Sent Events scoped to the request: notifications about it, then
//!   its response. There are no sessions.
//! - **Streamable HTTP, 2025-03-26 to 2025-11-25.** The `initialize`
//!   handshake opens a session the server names in `Mcp-Session-Id`, sent
//!   with every later request (with the agreed `MCP-Protocol-Version` from
//!   2025-06-18 on); a server may send requests of its own on a response's
//!   stream, which are answered with a `POST`; a session the server forgot
//!   (`404`) is opened again; it is closed with `DELETE`.
//! - **HTTP+SSE, 2024-11-05** (deprecated): a `GET` opens a stream whose
//!   first event names, on the same origin, where messages are posted, and
//!   every answer comes on that stream.
//!
//! The era is found the way the specification says: a modern request
//! first, a `4xx` without a modern error falling back to the handshake,
//! and a handshake refused the same way falling back to HTTP+SSE.
//! Requests carry the `Authorization` an [`Authorizer`] gives; a `401`, or
//! a `403` asking for more scopes, is answered by renewing it once, with no
//! person, and otherwise refuses the call with what to do.

use std::io::{BufRead, BufReader, Read};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use crate::mcp::{Failure, MAX_MESSAGE, MODERN};
use crate::oauth::{challenges, Authorizer};

/// The JSON-RPC errors that only a modern server gives, by which it is
/// told from an earlier one.
pub const MODERN_ERRORS: [i64; 3] = [-32020, -32021, -32022];

/// The code of the refusal of a request the server would not authorize.
pub const UNAUTHORIZED: i64 = -32_401;

/// One Server-Sent Event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    /// Its type (`message` unless it says).
    pub name: String,
    /// Its data, lines joined by line feeds.
    pub data: String,
}

/// Reads Server-Sent Events from a stream, each of at most `max` bytes.
pub struct Events<R: Read> {
    reader: BufReader<R>,
    max: usize,
    stop: Option<Arc<AtomicBool>>,
}

impl<R: Read> std::fmt::Debug for Events<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Events").field("max", &self.max).finish()
    }
}

impl<R: Read> Events<R> {
    /// Events from `reader`.
    pub fn new(reader: R, max: usize) -> Events<R> {
        Events {
            reader: BufReader::new(reader),
            max,
            stop: None,
        }
    }

    /// The same events, ending as the stream would at the first line read
    /// (a comment included) once `stop` is set.
    pub fn until(mut self, stop: Arc<AtomicBool>) -> Events<R> {
        self.stop = Some(stop);
        self
    }

    fn stopped(&self) -> bool {
        self.stop.as_ref().is_some_and(|s| s.load(Ordering::SeqCst))
    }

    /// The next event; `None` when the stream ends.
    pub fn next_event(&mut self) -> Option<Result<Event, String>> {
        let mut name = String::new();
        let mut data: Option<String> = None;
        let mut size = 0usize;
        loop {
            if self.stopped() {
                return None;
            }
            let mut raw = Vec::new();
            match self.reader.read_until(b'\n', &mut raw) {
                Ok(0) => return None,
                Ok(_) => {}
                Err(e) => return Some(Err(e.to_string())),
            }
            if self.stopped() {
                return None;
            }
            size += raw.len();
            if size > self.max {
                return Some(Err(format!("an event longer than {} bytes", self.max)));
            }
            let line = String::from_utf8_lossy(&raw);
            let line = line.trim_end_matches(['\n', '\r']);
            if line.is_empty() {
                match data.take() {
                    Some(data) => {
                        return Some(Ok(Event {
                            name: if name.is_empty() {
                                "message".into()
                            } else {
                                name
                            },
                            data,
                        }))
                    }
                    None => {
                        name.clear();
                        size = 0;
                        continue;
                    }
                }
            }
            if line.starts_with(':') {
                continue;
            }
            let (field, value) = match line.split_once(':') {
                Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
                None => (line, ""),
            };
            match field {
                "event" => name = value.to_string(),
                "data" => match &mut data {
                    Some(data) => {
                        data.push('\n');
                        data.push_str(value);
                    }
                    None => data = Some(value.to_string()),
                },
                _ => {}
            }
        }
    }
}

/// A header value as the transport carries it: as it is when it is plain
/// printable ASCII without spaces at its ends, otherwise (or when it looks
/// like an encoded one) `=?base64?…?=`.
pub fn header_value(text: &str) -> String {
    let plain = text
        .bytes()
        .all(|b| (0x20..0x7f).contains(&b) || b == b'\t')
        && !text.starts_with([' ', '\t'])
        && !text.ends_with([' ', '\t'])
        && !(text.starts_with("=?base64?") && text.ends_with("?="));
    if plain {
        text.to_string()
    } else {
        format!("=?base64?{}?=", crate::encoding::base64(text.as_bytes()))
    }
}

fn token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c)
}

/// The arguments a tool's input schema mirrors into headers
/// (`x-mcp-header`): each one's header name and the path of properties
/// that leads to it. A schema whose annotations break the rules (not on a
/// string, integer or boolean property reachable through `properties`
/// alone, not a token, not unique ignoring case) is refused: the tool is
/// not offered.
pub fn mirrored(schema: &Value) -> Result<Vec<(String, Vec<String>)>, String> {
    fn walk(
        node: &Value,
        path: &mut Vec<String>,
        reachable: bool,
        out: &mut Vec<(String, Vec<String>)>,
    ) -> Result<(), String> {
        let Value::Object(map) = node else {
            return Ok(());
        };
        if let Some(name) = map.get("x-mcp-header") {
            let name = name.as_str().unwrap_or_default();
            if !reachable || path.is_empty() {
                return Err(format!(
                    "x-mcp-header {name:?} is on a value not reached through properties alone"
                ));
            }
            if name.is_empty() || !name.chars().all(token_char) {
                return Err(format!("x-mcp-header {name:?} is not a header name"));
            }
            let kind = map.get("type").and_then(Value::as_str).unwrap_or_default();
            if !matches!(kind, "string" | "integer" | "boolean") {
                return Err(format!(
                    "x-mcp-header {name:?} is on a {kind:?} value, not a string, integer or boolean"
                ));
            }
            if out
                .iter()
                .any(|(other, _)| other.eq_ignore_ascii_case(name))
            {
                return Err(format!("x-mcp-header {name:?} is given twice"));
            }
            out.push((name.to_string(), path.clone()));
        }
        for (key, value) in map {
            match key.as_str() {
                "properties" => {
                    if let Value::Object(properties) = value {
                        for (property, schema) in properties {
                            path.push(property.clone());
                            walk(schema, path, reachable, out)?;
                            path.pop();
                        }
                    }
                }
                "x-mcp-header" | "type" | "description" | "title" | "enum" | "const"
                | "default" | "examples" | "required" => {}
                _ => match value {
                    Value::Array(items) => {
                        for item in items {
                            walk(item, path, false, out)?;
                        }
                    }
                    Value::Object(_) => walk(value, path, false, out)?,
                    _ => {}
                },
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(schema, &mut Vec::new(), true, &mut out)?;
    Ok(out)
}

/// The `Mcp-Param-*` headers of a call with `arguments`, for the
/// arguments `mirrored` names; an argument absent or null gets none.
pub fn param_headers(
    mirrored: &[(String, Vec<String>)],
    arguments: &Value,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (name, path) in mirrored {
        let mut value = arguments;
        for key in path {
            value = &value[key];
        }
        let text = match value {
            Value::String(s) => s.clone(),
            Value::Bool(b) => b.to_string(),
            Value::Number(n) if n.is_i64() || n.is_u64() => n.to_string(),
            _ => continue,
        };
        out.push((format!("Mcp-Param-{name}"), header_value(&text)));
    }
    out
}

/// What a POST got back: its status and the JSON-RPC message that answers
/// it, if one did.
#[derive(Clone, Debug, PartialEq)]
pub struct Posted {
    /// The HTTP status.
    pub status: u16,
    /// The answer.
    pub message: Option<Value>,
}

/// Whether `posted` is a modern server's refusal ([`MODERN_ERRORS`]).
pub fn modern_refusal(posted: &Posted) -> bool {
    posted
        .message
        .as_ref()
        .and_then(|m| m["error"]["code"].as_i64())
        .is_some_and(|code| MODERN_ERRORS.contains(&code))
}

/// A link to a server over Streamable HTTP.
pub struct Http {
    server: String,
    url: String,
    client: sion_http::Client,
    headers: Vec<(String, String)>,
    auth: Option<Arc<dyn Authorizer>>,
    /// The session of an earlier-era server.
    pub session: Option<String>,
    /// The version an earlier-era server agreed to, for its header.
    pub version: Option<String>,
    /// Whether requests are modern ones.
    pub modern: bool,
    /// The session the server forgot, if it said so (`404`).
    pub expired: bool,
}

impl std::fmt::Debug for Http {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Http")
            .field("server", &self.server)
            .field("url", &self.url)
            .field("modern", &self.modern)
            .field("session", &self.session.as_ref().map(|_| "…"))
            .finish()
    }
}

/// The headers a server's configuration gives (`Name: value`, `${VAR}`
/// replaced by the environment variable's value where the agent runs).
pub fn configured_headers(server: &str, lines: &[String]) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    for line in lines {
        let (name, template) = line
            .split_once(':')
            .ok_or_else(|| format!("{line:?} is not a header: give Name: value"))?;
        let name = name.trim();
        if name.is_empty() || !name.chars().all(token_char) {
            return Err(format!("{name:?} is not a header name"));
        }
        let mut value = String::new();
        let mut rest = template.trim();
        while let Some(start) = rest.find("${") {
            value.push_str(&rest[..start]);
            let after = &rest[start + 2..];
            let end = after
                .find('}')
                .ok_or_else(|| format!("{line:?} opens ${{ without closing it"))?;
            let var = &after[..end];
            let found = std::env::var(var).map_err(|_| {
                format!("the MCP server {server} needs the environment variable {var}, which is not set")
            })?;
            value.push_str(&found);
            rest = &after[end + 1..];
        }
        value.push_str(rest);
        if value.bytes().any(|b| b == b'\r' || b == b'\n') {
            return Err(format!("the header {name} would hold a line break"));
        }
        out.push((name.to_string(), value));
    }
    Ok(out)
}

/// The `Authorization` header `auth` gives now, if any.
fn authorization(auth: Option<&Arc<dyn Authorizer>>) -> Result<Option<String>, Failure> {
    match auth {
        Some(auth) => auth.header().map_err(|why| Failure::Refused {
            code: UNAUTHORIZED,
            message: why,
            data: Value::Null,
        }),
        None => Ok(None),
    }
}

/// What a response says about authorization: `Ok(None)` when it is not
/// about it, `Ok(Some(header))` to send again with a renewed one (once,
/// which `renewed` records), or the refusal, saying what to do. A `401`
/// asks for it, and so does a `403` whose challenge asks for more scopes.
fn reauthorize(
    server: &str,
    auth: Option<&Arc<dyn Authorizer>>,
    renewed: &mut bool,
    response: &sion_http::Response,
) -> Result<Option<String>, Failure> {
    let status = response.status;
    if status != 401 && status != 403 {
        return Ok(None);
    }
    let found = challenges(response.all("www-authenticate"));
    let wants_scope = found
        .iter()
        .any(|c| c.param("error") == Some("insufficient_scope"));
    if status == 403 && !wants_scope {
        return Ok(None);
    }
    let why = match (auth, *renewed) {
        (Some(auth), false) => match auth.renew(status, &found) {
            Ok(header) => {
                *renewed = true;
                return Ok(Some(header));
            }
            Err(why) => why,
        },
        (Some(_), true) => format!(
            "the MCP server {server} refused the renewed authorization (HTTP {status})"
        ),
        (None, _) => format!(
            "the MCP server {server} needs authorization (HTTP {status}): log in with `sion agent mcp login {server}`"
        ),
    };
    Err(Failure::Refused {
        code: UNAUTHORIZED,
        message: why,
        data: Value::Null,
    })
}

impl Http {
    /// A link to `url`, with these headers and authorization, whose
    /// requests may take `wait`.
    pub fn new(
        server: &str,
        url: &str,
        headers: Vec<(String, String)>,
        auth: Option<Arc<dyn Authorizer>>,
        wait: Duration,
    ) -> Result<Http, String> {
        if !url.starts_with("https://") && !url.starts_with("http://") {
            return Err(format!("{url} is not an http or https URL"));
        }
        // The endpoint is the operator's: it may be on this machine or the
        // local network.
        let client = sion_http::Client::new(sion_http::Options {
            allow_private: true,
            timeout: wait + Duration::from_secs(5),
            ..sion_http::Options::default()
        })
        .map_err(|e| e.to_string())?;
        Ok(Http {
            server: server.to_string(),
            url: url.to_string(),
            client,
            headers,
            auth,
            session: None,
            version: None,
            modern: true,
            expired: false,
        })
    }

    fn base_headers(&self, authorization: &Option<String>) -> Vec<(String, String)> {
        let mut out = vec![
            ("Content-Type".to_string(), "application/json".to_string()),
            (
                "Accept".to_string(),
                "application/json, text/event-stream".to_string(),
            ),
        ];
        out.extend(self.headers.iter().cloned());
        if let Some(auth) = authorization {
            out.push(("Authorization".into(), auth.clone()));
        }
        out
    }

    /// The headers a modern request mirrors from its body.
    fn mirrors(message: &Value) -> Vec<(String, String)> {
        let mut out = vec![("MCP-Protocol-Version".to_string(), MODERN.to_string())];
        if let Some(method) = message["method"].as_str() {
            out.push(("Mcp-Method".into(), method.to_string()));
            let name = match method {
                "tools/call" | "prompts/get" => message["params"]["name"].as_str(),
                "resources/read" => message["params"]["uri"].as_str(),
                _ => None,
            };
            if let Some(name) = name {
                out.push(("Mcp-Name".into(), header_value(name)));
            }
        }
        out
    }

    fn authorization(&self) -> Result<Option<String>, Failure> {
        authorization(self.auth.as_ref())
    }

    /// Posts `message` (a request, a notification or an answer to the
    /// server's request) with `extra` headers, and reads what answers it:
    /// for a request with id `id`, its response, from a JSON body or the
    /// request's event stream, answering on the way the requests an
    /// earlier-era server sends there.
    pub fn post(
        &mut self,
        message: &Value,
        id: Option<u64>,
        extra: &[(String, String)],
    ) -> Result<Posted, Failure> {
        let mut authorization = self.authorization()?;
        let mut renewed = false;
        loop {
            let mut headers = self.base_headers(&authorization);
            if self.modern {
                headers.extend(Self::mirrors(message));
            } else {
                if let Some(session) = &self.session {
                    headers.push(("Mcp-Session-Id".into(), session.clone()));
                }
                if let Some(version) = self.version.as_ref().filter(|v| v.as_str() >= "2025-06-18")
                {
                    headers.push(("MCP-Protocol-Version".into(), version.clone()));
                }
            }
            headers.extend(extra.iter().cloned());
            let pairs: Vec<(&str, &str)> = headers
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            let body = message.to_string();
            let mut response = self
                .client
                .send("POST", &self.url, &pairs, Some(body.as_bytes()))
                .map_err(|e| Failure::Lost(format!("the MCP server {}: {e}", self.server)))?;
            let status = response.status;
            if let Some(header) =
                reauthorize(&self.server, self.auth.as_ref(), &mut renewed, &response)?
            {
                authorization = Some(header);
                continue;
            }
            if !self.modern && status == 404 && self.session.is_some() {
                self.expired = true;
                self.session = None;
            }
            if let Some(session) = response.header("mcp-session-id") {
                if !self.modern && !session.is_empty() {
                    self.session = Some(session.to_string());
                }
            }
            let media = response.media_type().unwrap_or_default();
            if media == "text/event-stream" {
                let found = self.read_stream(response.body(), id)?;
                return Ok(Posted {
                    status,
                    message: found,
                });
            }
            let bytes = response
                .read_all(MAX_MESSAGE as u64)
                .map_err(|e| Failure::Lost(format!("the MCP server {}: {e}", self.server)))?;
            let parsed: Option<Value> = serde_json::from_slice(&bytes).ok();
            let found = match parsed {
                Some(Value::Array(items)) => items
                    .into_iter()
                    .find(|m| id.is_some() && m["id"].as_u64() == id),
                Some(value @ Value::Object(_)) => Some(value),
                _ => None,
            };
            return Ok(Posted {
                status,
                message: found,
            });
        }
    }

    /// Reads a request's event stream until its response.
    fn read_stream(
        &mut self,
        body: &mut Box<dyn Read + Send + Sync>,
        id: Option<u64>,
    ) -> Result<Option<Value>, Failure> {
        let mut events = Events::new(body, MAX_MESSAGE);
        while let Some(event) = events.next_event() {
            let event =
                event.map_err(|e| Failure::Lost(format!("the MCP server {}: {e}", self.server)))?;
            if event.name != "message" {
                continue;
            }
            let Ok(message) = serde_json::from_str::<Value>(&event.data) else {
                continue;
            };
            match (message.get("method"), message.get("id")) {
                // A request of an earlier-era server: answered at once.
                (Some(method), Some(their)) => {
                    let answer = answer_to(method.as_str().unwrap_or_default(), their);
                    let _ = self.post(&answer, None, &[]);
                }
                (Some(_), None) => {}
                (None, Some(their)) if id.is_some() && their.as_u64() == id => {
                    return Ok(Some(message));
                }
                _ => {}
            }
        }
        if id.is_none() {
            return Ok(None);
        }
        Err(Failure::Lost(format!(
            "the MCP server {} closed the stream before answering",
            self.server
        )))
    }

    /// Ends an earlier-era server's session (`DELETE`), if it has one.
    pub fn close(&mut self) {
        if let Some(session) = self.session.take() {
            let mut headers: Vec<(String, String)> = self.headers.clone();
            headers.push(("Mcp-Session-Id".into(), session));
            if let Ok(Some(auth)) = self.authorization() {
                headers.push(("Authorization".into(), auth));
            }
            let pairs: Vec<(&str, &str)> = headers
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            let _ = self.client.send("DELETE", &self.url, &pairs, None);
        }
    }
}

/// This client's answer to a request a server sends it.
pub fn answer_to(method: &str, id: &Value) -> Value {
    match method {
        "ping" => json!({"jsonrpc": "2.0", "id": id, "result": {}}),
        "roots/list" => json!({"jsonrpc": "2.0", "id": id, "result": {"roots": []}}),
        _ => json!({"jsonrpc": "2.0", "id": id, "error": {
            "code": -32601, "message": "this client does not offer that"
        }}),
    }
}

/// The deprecated HTTP+SSE transport (2024-11-05): a stream that carries
/// every answer, and an endpoint, named on it, where messages are posted.
///
/// The stream is read on a thread of its own. When the link is dropped,
/// that thread stops at the next line the server sends (servers send a
/// comment every few seconds to keep a stream alive) and closes the
/// connection; a server that sends nothing more keeps it until it closes
/// it itself, or a week has passed.
pub struct Sse {
    server: String,
    post_url: String,
    client: sion_http::Client,
    headers: Vec<(String, String)>,
    auth: Option<Arc<dyn Authorizer>>,
    closed: Arc<AtomicBool>,
    /// What the stream carries, as it comes.
    pub incoming: Receiver<Value>,
}

impl Drop for Sse {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::SeqCst);
    }
}

impl std::fmt::Debug for Sse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sse")
            .field("server", &self.server)
            .field("post_url", &self.post_url)
            .finish()
    }
}

/// `url`'s origin (`scheme://authority`).
fn origin(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    Some(format!(
        "{}://{}",
        scheme.to_ascii_lowercase(),
        authority.to_ascii_lowercase()
    ))
}

impl Sse {
    /// Opens the stream at `url` and waits, at most `wait`, for the event
    /// that names where to post; that must be on the same origin. A stream
    /// refused for want of authorization is asked for again once, with
    /// what `auth` renews.
    pub fn open(
        server: &str,
        url: &str,
        headers: Vec<(String, String)>,
        auth: Option<Arc<dyn Authorizer>>,
        wait: Duration,
    ) -> Result<Sse, Failure> {
        let lost = |e: String| Failure::Lost(format!("the MCP server {server}: {e}"));
        // The stream lasts as long as the connection.
        let long = sion_http::Client::new(sion_http::Options {
            allow_private: true,
            timeout: Duration::from_secs(7 * 24 * 3600),
            ..sion_http::Options::default()
        })
        .map_err(|e| lost(e.to_string()))?;
        let client = sion_http::Client::new(sion_http::Options {
            allow_private: true,
            timeout: wait + Duration::from_secs(5),
            ..sion_http::Options::default()
        })
        .map_err(|e| lost(e.to_string()))?;
        let mut authorization = authorization(auth.as_ref())?;
        let mut renewed = false;
        let mut response = loop {
            let mut all = headers.clone();
            all.push(("Accept".into(), "text/event-stream".into()));
            if let Some(header) = &authorization {
                all.push(("Authorization".into(), header.clone()));
            }
            let pairs: Vec<(&str, &str)> =
                all.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
            let response = long
                .send("GET", url, &pairs, None)
                .map_err(|e| lost(e.to_string()))?;
            match reauthorize(server, auth.as_ref(), &mut renewed, &response)? {
                Some(header) => authorization = Some(header),
                None => break response,
            }
        };
        if response.status != 200 || response.media_type().as_deref() != Some("text/event-stream") {
            return Err(Failure::Refused {
                code: 0,
                message: format!(
                    "the MCP server {server} opened no event stream (HTTP {})",
                    response.status
                ),
                data: json!({"status": response.status}),
            });
        }
        let (sender, incoming) = mpsc::channel();
        let (found, endpoint) = mpsc::channel();
        let closed = Arc::new(AtomicBool::new(false));
        let stop = closed.clone();
        let name = server.to_string();
        std::thread::Builder::new()
            .name(format!("mcp-sse-{name}"))
            .spawn(move || {
                let mut events = Events::new(response.body(), MAX_MESSAGE).until(stop);
                while let Some(Ok(event)) = events.next_event() {
                    match event.name.as_str() {
                        "endpoint" => {
                            let _ = found.send(event.data.trim().to_string());
                        }
                        "message" => {
                            if let Ok(message) = serde_json::from_str::<Value>(&event.data) {
                                if sender.send(message).is_err() {
                                    break;
                                }
                            }
                        }
                        _ => {}
                    }
                }
            })
            .map_err(|e| lost(e.to_string()))?;
        // From here, a stream that is not used is let go.
        let refuse = |why: String| {
            closed.store(true, Ordering::SeqCst);
            Failure::Refused {
                code: 0,
                message: why,
                data: Value::Null,
            }
        };
        let named = endpoint.recv_timeout(wait).map_err(|_| {
            refuse(format!(
                "the MCP server {server}'s event stream named no endpoint to post to"
            ))
        })?;
        let base = origin(url).ok_or_else(|| refuse(format!("{url} has no origin")))?;
        let post_url = if named.starts_with("http://") || named.starts_with("https://") {
            named
        } else if named.starts_with('/') {
            format!("{base}{named}")
        } else {
            let dir = url.rsplit_once('/').map(|(d, _)| d).unwrap_or(url);
            format!("{dir}/{named}")
        };
        if origin(&post_url).as_deref() != Some(base.as_str()) {
            return Err(refuse(format!(
                "the MCP server {server} named an endpoint on another origin ({post_url}): it is not used"
            )));
        }
        Ok(Sse {
            server: server.to_string(),
            post_url,
            client,
            headers,
            auth,
            closed,
            incoming,
        })
    }

    /// Posts a message; its answer, if any, comes on the stream. A message
    /// refused for want of authorization is posted again once, with what
    /// the link's authorizer renews; any other refusal (`4xx`) did nothing,
    /// and a server's failure (`5xx`) may or may not have.
    pub fn send(&self, message: &Value) -> Result<(), Failure> {
        let body = message.to_string();
        let mut authorization = authorization(self.auth.as_ref())?;
        let mut renewed = false;
        loop {
            let mut all = self.headers.clone();
            all.push(("Content-Type".into(), "application/json".into()));
            if let Some(header) = &authorization {
                all.push(("Authorization".into(), header.clone()));
            }
            let pairs: Vec<(&str, &str)> =
                all.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
            let response = self
                .client
                .send("POST", &self.post_url, &pairs, Some(body.as_bytes()))
                .map_err(|e| Failure::Lost(format!("the MCP server {}: {e}", self.server)))?;
            if let Some(header) =
                reauthorize(&self.server, self.auth.as_ref(), &mut renewed, &response)?
            {
                authorization = Some(header);
                continue;
            }
            let status = response.status;
            let why = format!(
                "the MCP server {} refused a message (HTTP {status})",
                self.server
            );
            return match status {
                200..=299 => Ok(()),
                400..=499 => Err(Failure::Refused {
                    code: 0,
                    message: why,
                    data: json!({"status": status}),
                }),
                _ => Err(Failure::Lost(why)),
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_are_read_as_the_specification_frames_them() {
        let stream = ": a comment\r\n\r\nevent: endpoint\ndata: /messages?s=1\n\ndata: {\"a\":\ndata: 1}\n\nid: 7\nretry: 10\n\nevent: message\r\ndata:x\r\n\r\n";
        let mut events = Events::new(stream.as_bytes(), 1024);
        assert_eq!(
            events.next_event().unwrap().unwrap(),
            Event {
                name: "endpoint".into(),
                data: "/messages?s=1".into()
            }
        );
        let second = events.next_event().unwrap().unwrap();
        assert_eq!(
            (second.name.as_str(), second.data.as_str()),
            ("message", "{\"a\":\n1}")
        );
        let third = events.next_event().unwrap().unwrap();
        assert_eq!(third.data, "x");
        assert!(events.next_event().is_none());
        let mut long = Events::new("data: 0123456789\n\n".as_bytes(), 8);
        assert!(long.next_event().unwrap().is_err());
    }

    #[test]
    fn header_values_are_encoded_when_they_cannot_be_plain() {
        assert_eq!(header_value("us-west1"), "us-west1");
        assert_eq!(
            header_value("Hello, 世界"),
            "=?base64?SGVsbG8sIOS4lueVjA==?="
        );
        assert_eq!(header_value(" padded "), "=?base64?IHBhZGRlZCA=?=");
        assert_eq!(header_value("line1\nline2"), "=?base64?bGluZTEKbGluZTI=?=");
        assert_eq!(
            header_value("=?base64?literal?="),
            "=?base64?PT9iYXNlNjQ/bGl0ZXJhbD89?="
        );
    }

    #[test]
    fn mirrored_arguments_follow_the_rules() {
        let schema = json!({
            "type": "object",
            "properties": {
                "region": {"type": "string", "x-mcp-header": "Region"},
                "query": {"type": "string"},
                "options": {"type": "object", "properties": {
                    "dry": {"type": "boolean", "x-mcp-header": "Dry-Run"},
                    "limit": {"type": "integer", "x-mcp-header": "Limit"}
                }}
            }
        });
        let found = mirrored(&schema).unwrap();
        assert_eq!(found.len(), 3);
        let headers = param_headers(
            &found,
            &json!({"region": "us-west1", "query": "SELECT 1", "options": {"dry": true, "limit": 5}}),
        );
        assert!(headers.contains(&("Mcp-Param-Region".into(), "us-west1".into())));
        assert!(headers.contains(&("Mcp-Param-Dry-Run".into(), "true".into())));
        assert!(headers.contains(&("Mcp-Param-Limit".into(), "5".into())));
        assert!(param_headers(&found, &json!({"region": null})).is_empty());
        for bad in [
            json!({"properties": {"n": {"type": "number", "x-mcp-header": "N"}}}),
            json!({"properties": {"a": {"type": "string", "x-mcp-header": "Bad Name"}}}),
            json!({"properties": {"a": {"type": "string", "x-mcp-header": "X"}, "b": {"type": "string", "x-mcp-header": "x"}}}),
            json!({"properties": {"list": {"type": "array", "items": {"type": "string", "x-mcp-header": "Item"}}}}),
            json!({"anyOf": [{"properties": {"a": {"type": "string", "x-mcp-header": "A"}}}]}),
        ] {
            assert!(mirrored(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn configured_headers_take_values_from_the_environment() {
        std::env::set_var("SION_TEST_MCP_TOKEN", "t0ken");
        let headers = configured_headers(
            "x",
            &[
                "Authorization: Bearer ${SION_TEST_MCP_TOKEN}".into(),
                "X-Team: blue".into(),
            ],
        )
        .unwrap();
        assert_eq!(headers[0], ("Authorization".into(), "Bearer t0ken".into()));
        assert_eq!(headers[1], ("X-Team".into(), "blue".into()));
        assert!(configured_headers("x", &["X: ${SION_TEST_MCP_MISSING}".into()]).is_err());
        assert!(configured_headers("x", &["no colon".into()]).is_err());
    }
}
