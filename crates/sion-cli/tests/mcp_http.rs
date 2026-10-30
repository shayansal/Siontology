//! MCP servers over HTTP, against a stand-in on this machine: Streamable
//! HTTP in the modern era (answers as JSON and as event streams, headers
//! mirrored from the body, a tool whose header annotations break the
//! rules left out), in the handshake era (a session, a request from the
//! server on a stream, a forgotten session opened again), and the
//! deprecated HTTP+SSE transport (its stream let go when the link is
//! dropped); and OAuth: discovery from the 401 (on the old transport, from
//! its stream's `GET`), dynamic registration, a browser's round trip
//! checked for its issuer and state, PKCE and `resource` at the token
//! endpoint, a refused token renewed with its refresh token (rotated), and
//! client credentials.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use sion_agent::mcp::{self, Connection, Server, MODERN};
use sion_agent::oauth::{self, Authorizer};
use sion_cli::mcp_auth::{AuthStore, OAuthConfig, Stored};

/// How the stand-in speaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Era {
    Modern,
    Legacy,
    Sse,
}

/// What the stand-in remembers.
#[derive(Default)]
struct State {
    /// Valid access tokens.
    access: Vec<String>,
    /// Valid refresh tokens.
    refresh: Vec<String>,
    /// Codes issued: code → (client, challenge, redirect, resource).
    codes: BTreeMap<String, (String, String, String, String)>,
    /// Registered clients.
    clients: Vec<String>,
    /// Sessions of the handshake era.
    sessions: Vec<String>,
    /// Requests seen, as `METHOD path mcp-method`.
    seen: Vec<String>,
    /// Messages waiting for the HTTP+SSE stream.
    outbox: BTreeMap<String, Vec<Value>>,
    /// Header mismatches seen.
    mismatches: Vec<String>,
    /// HTTP+SSE streams open now.
    streams: usize,
}

struct Stand {
    port: u16,
    era: Era,
    auth: bool,
    state: Mutex<State>,
    issued: AtomicUsize,
    forget_sessions: AtomicBool,
}

impl Stand {
    fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn issuer(&self) -> String {
        format!("{}/auth", self.base())
    }

    fn next(&self, prefix: &str) -> String {
        format!(
            "{prefix}-{}",
            self.issued.fetch_add(1, Ordering::SeqCst) + 1
        )
    }
}

fn header(request: &tiny_http::Request, name: &str) -> Option<String> {
    request
        .headers()
        .iter()
        .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name))
        .map(|h| h.value.as_str().to_string())
}

fn respond_json(request: tiny_http::Request, status: u16, body: &Value, extra: &[(&str, String)]) {
    let mut response = tiny_http::Response::from_string(body.to_string())
        .with_status_code(status)
        .with_header(
            "Content-Type: application/json"
                .parse::<tiny_http::Header>()
                .unwrap(),
        );
    for (name, value) in extra {
        response = response
            .with_header(tiny_http::Header::from_bytes(name.as_bytes(), value.as_bytes()).unwrap());
    }
    let _ = request.respond(response);
}

fn respond_events(request: tiny_http::Request, events: &[String], extra: &[(&str, String)]) {
    let body: String = events.concat();
    let mut response = tiny_http::Response::from_string(body).with_header(
        "Content-Type: text/event-stream"
            .parse::<tiny_http::Header>()
            .unwrap(),
    );
    for (name, value) in extra {
        response = response
            .with_header(tiny_http::Header::from_bytes(name.as_bytes(), value.as_bytes()).unwrap());
    }
    let _ = request.respond(response);
}

fn event(message: &Value) -> String {
    format!("event: message\ndata: {message}\n\n")
}

fn tools() -> Value {
    json!([
        {"name": "echo", "description": "Says the text back.",
         "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}},
         "annotations": {"readOnlyHint": true, "openWorldHint": false}},
        {"name": "query", "description": "Runs a query in a region.",
         "inputSchema": {"type": "object", "properties": {
             "region": {"type": "string", "x-mcp-header": "Region"},
             "sql": {"type": "string"}}},
         "annotations": {"readOnlyHint": true, "openWorldHint": false}},
        {"name": "broken", "description": "Mirrors a number, which is not allowed.",
         "inputSchema": {"type": "object", "properties": {
             "n": {"type": "number", "x-mcp-header": "N"}}}}
    ])
}

fn call_result(params: &Value, era: Era) -> Value {
    let name = params["name"].as_str().unwrap_or_default();
    let text = match name {
        "echo" => format!(
            "echo: {}",
            params["arguments"]["text"].as_str().unwrap_or_default()
        ),
        "query" => format!(
            "{} rows in {}",
            3,
            params["arguments"]["region"].as_str().unwrap_or_default()
        ),
        _ => "unknown".into(),
    };
    let mut result = json!({"content": [{"type": "text", "text": text}], "isError": false});
    if era == Era::Modern {
        result["resultType"] = json!("complete");
    }
    result
}

/// `request` back if the stand-in asks for no authorization or it carries
/// a valid token; otherwise it is answered `401`, with the challenge.
fn authorized(stand: &Stand, request: tiny_http::Request) -> Option<tiny_http::Request> {
    if !stand.auth {
        return Some(request);
    }
    let token = header(&request, "authorization")
        .and_then(|v| v.strip_prefix("Bearer ").map(str::to_string));
    if token.is_some_and(|t| stand.state.lock().unwrap().access.contains(&t)) {
        return Some(request);
    }
    let challenge = format!(
        "Bearer resource_metadata=\"{}/.well-known/oauth-protected-resource/mcp\", scope=\"tools:read\"",
        stand.base()
    );
    respond_json(
        request,
        401,
        &json!({"error": "invalid_token"}),
        &[("WWW-Authenticate", challenge)],
    );
    None
}

/// The MCP endpoint, in the stand-in's era.
fn mcp_endpoint(stand: &Stand, mut request: tiny_http::Request) {
    let mut body = String::new();
    let _ = request.as_reader().read_to_string(&mut body);
    let message: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    let method = message["method"].as_str().unwrap_or_default().to_string();
    stand
        .state
        .lock()
        .unwrap()
        .seen
        .push(format!("POST /mcp {method}"));
    if stand.era == Era::Sse {
        // The deprecated transport has no endpoint that takes a POST; as
        // servers route requests, that is found before authorization.
        let _ = request.respond(tiny_http::Response::from_string("").with_status_code(405));
        return;
    }
    let Some(request) = authorized(stand, request) else {
        return;
    };
    let id = message["id"].clone();
    match stand.era {
        Era::Modern => {
            // The headers must mirror the body.
            let version = header(&request, "mcp-protocol-version");
            let mirrored = header(&request, "mcp-method");
            let mut mismatch = None;
            if version.as_deref() != Some(MODERN)
                || message["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"]
                    != json!(MODERN)
            {
                mismatch = Some("not a modern request".to_string());
            } else if mirrored.as_deref() != Some(method.as_str()) {
                mismatch = Some(format!("Mcp-Method {mirrored:?} is not {method}"));
            } else if method == "tools/call" {
                let name = message["params"]["name"].as_str().unwrap_or_default();
                if header(&request, "mcp-name").as_deref() != Some(name) {
                    mismatch = Some("Mcp-Name does not match".into());
                }
                if name == "query" {
                    let region = message["params"]["arguments"]["region"].as_str();
                    if header(&request, "mcp-param-region").as_deref() != region {
                        mismatch = Some("Mcp-Param-Region does not match".into());
                    }
                }
            }
            if let Some(why) = mismatch {
                if method == "initialize" {
                    // A modern-only server refuses the handshake.
                    return respond_json(
                        request,
                        400,
                        &json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32022,
                            "message": "Unsupported protocol version",
                            "data": {"supported": [MODERN]}}}),
                        &[],
                    );
                }
                stand.state.lock().unwrap().mismatches.push(why.clone());
                return respond_json(
                    request,
                    400,
                    &json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32020, "message": why}}),
                    &[],
                );
            }
            match method.as_str() {
                "server/discover" => respond_json(
                    request,
                    200,
                    &json!({"jsonrpc": "2.0", "id": id, "result": {
                        "resultType": "complete",
                        "supportedVersions": [MODERN],
                        "capabilities": {"tools": {}},
                        "_meta": {"io.modelcontextprotocol/serverInfo": {"name": "stand-in", "version": "1.0"}}
                    }}),
                    &[],
                ),
                "tools/list" => respond_json(
                    request,
                    200,
                    &json!({"jsonrpc": "2.0", "id": id, "result": {"resultType": "complete", "tools": tools()}}),
                    &[],
                ),
                "tools/call" => {
                    // Answered on an event stream, a progress note first.
                    let progress = json!({"jsonrpc": "2.0", "method": "notifications/progress",
                        "params": {"progressToken": 1, "progress": 1}});
                    let answer = json!({"jsonrpc": "2.0", "id": id,
                        "result": call_result(&message["params"], Era::Modern)});
                    respond_events(
                        request,
                        &[": keep-alive\n\n".into(), event(&progress), event(&answer)],
                        &[],
                    );
                }
                _ => respond_json(
                    request,
                    404,
                    &json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "method not found"}}),
                    &[],
                ),
            }
        }
        Era::Legacy => {
            let session = header(&request, "mcp-session-id");
            if method == "initialize" {
                let session = stand.next("session");
                stand.state.lock().unwrap().sessions.push(session.clone());
                return respond_json(
                    request,
                    200,
                    &json!({"jsonrpc": "2.0", "id": id, "result": {
                        "protocolVersion": "2025-11-25",
                        "capabilities": {"tools": {}},
                        "serverInfo": {"name": "stand-in", "version": "0.9"}}}),
                    &[("Mcp-Session-Id", session)],
                );
            }
            let known = session
                .as_ref()
                .is_some_and(|s| stand.state.lock().unwrap().sessions.contains(s));
            if session.is_none() {
                return respond_json(
                    request,
                    400,
                    &json!({"jsonrpc": "2.0", "id": Value::Null, "error": {"code": -32000,
                        "message": "Bad Request: No valid session ID provided"}}),
                    &[],
                );
            }
            if !known || stand.forget_sessions.swap(false, Ordering::SeqCst) {
                stand.state.lock().unwrap().sessions.clear();
                let _ = request.respond(tiny_http::Response::from_string("").with_status_code(404));
                return;
            }
            if header(&request, "mcp-protocol-version").as_deref() != Some("2025-11-25") {
                return respond_json(
                    request,
                    400,
                    &json!({"error": "no protocol version header"}),
                    &[],
                );
            }
            match method.as_str() {
                "notifications/initialized" | "" => {
                    let _ =
                        request.respond(tiny_http::Response::from_string("").with_status_code(202));
                }
                "tools/list" => {
                    // A request of the server's own, before the answer.
                    let ping = json!({"jsonrpc": "2.0", "id": "srv-1", "method": "ping"});
                    let answer = json!({"jsonrpc": "2.0", "id": id, "result": {"tools": tools()}});
                    respond_events(request, &[event(&ping), event(&answer)], &[]);
                }
                "tools/call" => respond_json(
                    request,
                    200,
                    &json!({"jsonrpc": "2.0", "id": id, "result": call_result(&message["params"], Era::Legacy)}),
                    &[],
                ),
                _ => respond_json(
                    request,
                    200,
                    &json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "method not found"}}),
                    &[],
                ),
            }
        }
        Era::Sse => unreachable!("answered above"),
    }
}

/// The HTTP+SSE stream, counted while it is open.
fn sse_stream(stand: Arc<Stand>, request: tiny_http::Request) {
    stand.state.lock().unwrap().streams += 1;
    stream_events(&stand, request);
    stand.state.lock().unwrap().streams -= 1;
}

/// The stream, written to the socket by hand so that each event goes out
/// at once: the endpoint, then the answers as they come, and a comment
/// every 100 ms of quiet, as servers keep their streams alive.
fn stream_events(stand: &Stand, request: tiny_http::Request) {
    use std::io::Write;
    let session = stand.next("s");
    let mut out = request.into_writer();
    let mut send = |text: &str| -> bool {
        out.write_all(text.as_bytes())
            .and_then(|()| out.flush())
            .is_ok()
    };
    if !send("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n")
        || !send(&format!("event: endpoint\ndata: /messages?session={session}\n\n"))
    {
        return;
    }
    let mut quiet = 0u32;
    loop {
        let next = {
            let mut state = stand.state.lock().unwrap();
            state
                .outbox
                .get_mut(&session)
                .and_then(|o| (!o.is_empty()).then(|| o.remove(0)))
        };
        match next {
            Some(message) => {
                quiet = 0;
                if !send(&event(&message)) {
                    return;
                }
            }
            None => {
                quiet += 1;
                if quiet % 20 == 0 && !send(": ping\n\n") {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
    }
}

fn sse_message(stand: &Stand, mut request: tiny_http::Request, query: &str) {
    let mut body = String::new();
    let _ = request.as_reader().read_to_string(&mut body);
    let message: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    let id = message["id"].clone();
    let answer = match message["method"].as_str() {
        Some("initialize") => Some(json!({"jsonrpc": "2.0", "id": id, "result": {
            "protocolVersion": "2024-11-05", "capabilities": {"tools": {}},
            "serverInfo": {"name": "old-stand-in", "version": "0.1"}}})),
        Some("tools/list") => {
            Some(json!({"jsonrpc": "2.0", "id": id, "result": {"tools": tools()}}))
        }
        Some("tools/call") => Some(json!({"jsonrpc": "2.0", "id": id,
            "result": call_result(&message["params"], Era::Sse)})),
        _ => None,
    };
    if let Some(answer) = answer {
        let session = oauth::query(query)
            .get("session")
            .cloned()
            .unwrap_or_default();
        stand
            .state
            .lock()
            .unwrap()
            .outbox
            .entry(session)
            .or_default()
            .push(answer);
    }
    let _ = request.respond(tiny_http::Response::from_string("Accepted").with_status_code(202));
}

/// The authorization server's endpoints.
fn authorization(stand: &Stand, mut request: tiny_http::Request, path: &str, query: &str) {
    let issuer = stand.issuer();
    match path {
        "/.well-known/oauth-protected-resource/mcp" => respond_json(
            request,
            200,
            &json!({"resource": format!("{}/mcp", stand.base()),
                    "authorization_servers": [issuer],
                    "scopes_supported": ["tools:read"]}),
            &[],
        ),
        "/.well-known/oauth-authorization-server/auth" => respond_json(
            request,
            200,
            &json!({"issuer": issuer,
                    "authorization_endpoint": format!("{issuer}/authorize"),
                    "token_endpoint": format!("{issuer}/token"),
                    "registration_endpoint": format!("{issuer}/register"),
                    "code_challenge_methods_supported": ["S256"],
                    "grant_types_supported": ["authorization_code", "refresh_token", "client_credentials"],
                    "authorization_response_iss_parameter_supported": true,
                    "revocation_endpoint": format!("{issuer}/revoke")}),
            &[],
        ),
        "/auth/register" => {
            let mut body = String::new();
            let _ = request.as_reader().read_to_string(&mut body);
            let asked: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
            let loopback = asked["redirect_uris"][0]
                .as_str()
                .is_some_and(|u| u.starts_with("http://127.0.0.1:"));
            if !loopback || asked["token_endpoint_auth_method"] != "none" {
                return respond_json(request, 400, &json!({"error": "invalid_redirect_uri"}), &[]);
            }
            let client = stand.next("client");
            stand.state.lock().unwrap().clients.push(client.clone());
            respond_json(request, 201, &json!({"client_id": client}), &[]);
        }
        "/auth/authorize" => {
            let q = oauth::query(query);
            let known = stand
                .state
                .lock()
                .unwrap()
                .clients
                .contains(&q["client_id"]);
            if !known || q["code_challenge_method"] != "S256" || !q.contains_key("resource") {
                let _ = request
                    .respond(tiny_http::Response::from_string("bad request").with_status_code(400));
                return;
            }
            let code = stand.next("code");
            stand.state.lock().unwrap().codes.insert(
                code.clone(),
                (
                    q["client_id"].clone(),
                    q["code_challenge"].clone(),
                    q["redirect_uri"].clone(),
                    q["resource"].clone(),
                ),
            );
            let location = format!(
                "{}?code={code}&state={}&iss={}",
                q["redirect_uri"],
                oauth::encode(&q["state"]),
                oauth::encode(&issuer)
            );
            let _ = request.respond(
                tiny_http::Response::from_string("")
                    .with_status_code(302)
                    .with_header(
                        tiny_http::Header::from_bytes("Location", location.as_bytes()).unwrap(),
                    ),
            );
        }
        "/auth/revoke" => {
            let mut body = String::new();
            let _ = request.as_reader().read_to_string(&mut body);
            let form = oauth::query(&body);
            let token = form.get("token").cloned().unwrap_or_default();
            {
                let mut state = stand.state.lock().unwrap();
                state.access.retain(|t| *t != token);
                state.refresh.retain(|t| *t != token);
            }
            let _ = request.respond(tiny_http::Response::from_string("").with_status_code(200));
        }
        "/auth/token" => {
            let mut body = String::new();
            let _ = request.as_reader().read_to_string(&mut body);
            let form = oauth::query(&body);
            let resource = format!("{}/mcp", stand.base());
            if form.get("resource") != Some(&resource) {
                return respond_json(request, 400, &json!({"error": "invalid_target"}), &[]);
            }
            let ok = match form.get("grant_type").map(String::as_str) {
                Some("authorization_code") => {
                    let issued = stand.state.lock().unwrap().codes.remove(&form["code"]);
                    match issued {
                        Some((client, challenge, redirect, _)) => {
                            let verifier = form.get("code_verifier").cloned().unwrap_or_default();
                            let computed = oauth::challenge_of(&verifier);
                            client == form["client_id"]
                                && computed == challenge
                                && redirect == form["redirect_uri"]
                        }
                        None => false,
                    }
                }
                Some("refresh_token") => {
                    let mut state = stand.state.lock().unwrap();
                    let token = form.get("refresh_token").cloned().unwrap_or_default();
                    let known = state.refresh.contains(&token);
                    // Rotated: the old one is spent.
                    state.refresh.retain(|t| *t != token);
                    known
                }
                Some("client_credentials") => {
                    header(&request, "authorization").as_deref()
                        == Some(&format!("Basic {}", base64_std(b"robot:s3cret"))[..])
                }
                _ => false,
            };
            if !ok {
                return respond_json(request, 400, &json!({"error": "invalid_grant"}), &[]);
            }
            let access = stand.next("access");
            let refresh = stand.next("refresh");
            {
                let mut state = stand.state.lock().unwrap();
                state.access.push(access.clone());
                state.refresh.push(refresh.clone());
            }
            respond_json(
                request,
                200,
                &json!({"access_token": access, "token_type": "Bearer", "expires_in": 3600,
                        "refresh_token": refresh, "scope": "tools:read"}),
                &[],
            );
        }
        _ => {
            let _ = request
                .respond(tiny_http::Response::from_string("not found").with_status_code(404));
        }
    }
}

fn base64_std(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn stand_in(era: Era, auth: bool) -> Arc<Stand> {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let port = server.server_addr().to_ip().unwrap().port();
    let stand = Arc::new(Stand {
        port,
        era,
        auth,
        state: Mutex::new(State::default()),
        issued: AtomicUsize::new(0),
        forget_sessions: AtomicBool::new(false),
    });
    let shared = stand.clone();
    std::thread::spawn(move || {
        for request in server.incoming_requests() {
            let stand = shared.clone();
            std::thread::spawn(move || {
                let url = request.url().to_string();
                let (path, query) = url.split_once('?').unwrap_or((&url, ""));
                let (path, query) = (path.to_string(), query.to_string());
                match (request.method(), path.as_str()) {
                    (tiny_http::Method::Post, "/mcp") => mcp_endpoint(&stand, request),
                    (tiny_http::Method::Get, "/mcp") if stand.era == Era::Sse => {
                        stand.state.lock().unwrap().seen.push("GET /mcp".into());
                        if let Some(request) = authorized(&stand, request) {
                            sse_stream(stand.clone(), request);
                        }
                    }
                    (tiny_http::Method::Delete, "/mcp") => {
                        stand.state.lock().unwrap().seen.push("DELETE /mcp".into());
                        let _ = request
                            .respond(tiny_http::Response::from_string("").with_status_code(200));
                    }
                    (tiny_http::Method::Post, "/messages") => {
                        stand
                            .state
                            .lock()
                            .unwrap()
                            .seen
                            .push("POST /messages".into());
                        if let Some(request) = authorized(&stand, request) {
                            sse_message(&stand, request, &query);
                        }
                    }
                    _ => authorization(&stand, request, &path, &query),
                }
            });
        }
    });
    stand
}

fn server(stand: &Stand) -> Server {
    Server {
        name: "remote".into(),
        command: Vec::new(),
        url: Some(format!("{}/mcp", stand.base())),
        headers: Vec::new(),
        oauth: None,
        legacy_sse: false,
        env: Vec::new(),
        sandbox: None,
        dir: None,
        tools: BTreeMap::new(),
        timeout_secs: 20,
        protocol: None,
    }
}

fn reviewed(mut server: Server, auth: Option<Arc<dyn Authorizer>>) -> Server {
    let mut connection = Connection::start_with(&server, auth).unwrap();
    server.protocol = Some(connection.protocol());
    server.legacy_sse = connection.transport() == "http+sse";
    for tool in connection.tools().unwrap() {
        let pin = tool.pin(None);
        server.tools.insert(tool.name.clone(), pin);
    }
    server
}

fn call(mounted: &mcp::Mounted, name: &str, input: Value) -> String {
    let tool = mounted
        .tools
        .iter()
        .find(|t| t.spec().name == name)
        .unwrap_or_else(|| panic!("no tool {name}: {:?}", mounted.notes));
    let outcome = tool.call(&input, "k");
    assert!(!outcome.is_error, "{outcome:?}");
    outcome.content
}

#[test]
fn a_modern_server_mirrors_its_requests_and_streams_its_answers() {
    let stand = stand_in(Era::Modern, false);
    let server = reviewed(server(&stand), None);
    assert_eq!(server.protocol.as_deref(), Some(MODERN));
    // The tool whose header annotation breaks the rules is not listed.
    assert!(server.tools.contains_key("query"));
    assert!(!server.tools.contains_key("broken"));
    let mounted = mcp::mount(&server).unwrap();
    assert!(
        mounted
            .notes
            .iter()
            .any(|n| n.contains("broken") && n.contains("not a string")),
        "{:?}",
        mounted.notes
    );
    assert_eq!(
        call(&mounted, "remote_echo", json!({"text": "hi"})),
        "echo: hi"
    );
    assert_eq!(
        call(
            &mounted,
            "remote_query",
            json!({"region": "eu-west1", "sql": "select 1"})
        ),
        "3 rows in eu-west1"
    );
    let state = stand.state.lock().unwrap();
    assert!(state.mismatches.is_empty(), "{:?}", state.mismatches);
    assert!(
        !state.seen.iter().any(|s| s.contains("initialize")),
        "{:?}",
        state.seen
    );
}

#[test]
fn a_server_of_the_handshake_era_keeps_a_session_and_opens_a_new_one() {
    let stand = stand_in(Era::Legacy, false);
    let server = reviewed(server(&stand), None);
    assert_eq!(server.protocol.as_deref(), Some("2025-11-25"));
    assert!(!server.legacy_sse);
    let mounted = mcp::mount(&server).unwrap();
    assert_eq!(
        call(&mounted, "remote_echo", json!({"text": "one"})),
        "echo: one"
    );
    // The server forgets the session: the client opens a new one and asks
    // again.
    stand.forget_sessions.store(true, Ordering::SeqCst);
    assert_eq!(
        call(&mounted, "remote_echo", json!({"text": "two"})),
        "echo: two"
    );
    let seen = stand.state.lock().unwrap().seen.clone();
    let handshakes = seen.iter().filter(|s| s.ends_with(" initialize")).count();
    // Reviewing, mounting, and once more after the server forgot.
    assert_eq!(handshakes, 3, "{seen:?}");
    drop(mounted);
    assert!(stand
        .state
        .lock()
        .unwrap()
        .seen
        .iter()
        .any(|s| s == "DELETE /mcp"));
}

/// Waits, at most 5 s, for the stand-in to have `open` streams open.
fn streams_open(stand: &Stand, open: usize) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if stand.state.lock().unwrap().streams == open {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    false
}

#[test]
fn a_server_of_the_old_http_sse_transport_is_reached_too() {
    let stand = stand_in(Era::Sse, false);
    let server = reviewed(server(&stand), None);
    assert!(server.legacy_sse);
    assert_eq!(server.protocol.as_deref(), Some("2024-11-05"));
    let mounted = mcp::mount(&server).unwrap();
    assert_eq!(
        call(&mounted, "remote_echo", json!({"text": "old"})),
        "echo: old"
    );
    // A link let go closes its stream, at the server's next comment: the
    // review's, then the session's.
    assert!(streams_open(&stand, 1), "the review's stream stayed open");
    drop(mounted);
    assert!(streams_open(&stand, 0), "the session's stream stayed open");
}

#[test]
fn an_old_http_sse_server_is_logged_in_to_from_its_streams_challenge() {
    let stand = stand_in(Era::Sse, true);
    let store = AuthStore::at(temp("sse-oauth"));
    let server = server(&stand);
    // The stream is refused, and the refusal says how to log in.
    let err = Connection::start(&server).unwrap_err();
    assert!(
        err.contains("needs authorization") && err.contains("sion agent mcp login remote"),
        "{err}"
    );
    // The POST is refused as one the URL does not take: the challenge is
    // the stream's.
    let url = server.url.clone().unwrap();
    let found = sion_cli::mcp_auth::first_challenge(&url, &[])
        .unwrap()
        .unwrap();
    assert!(found.iter().any(|c| c.param("resource_metadata").is_some()));
    let show = |url: &str| {
        let url = url.to_string();
        std::thread::spawn(move || browse(&url));
    };
    sion_cli::mcp_auth::login(&server, &OAuthConfig::default(), &store, None, &show).unwrap();
    let auth: Arc<dyn Authorizer> =
        Arc::new(Stored::new(&server, OAuthConfig::default(), store.clone()));
    let server = reviewed(server, Some(auth.clone()));
    assert!(server.legacy_sse);
    let mounted = mcp::mount_with(&server, Some(auth)).unwrap();
    assert_eq!(
        call(&mounted, "remote_echo", json!({"text": "old"})),
        "echo: old"
    );
    // The server stops taking the token: the message is posted again with
    // a renewed one, and its answer comes on the stream already open.
    let before = store.get("remote").unwrap().unwrap().tokens.unwrap();
    stand.state.lock().unwrap().access.clear();
    assert_eq!(
        call(&mounted, "remote_echo", json!({"text": "again"})),
        "echo: again"
    );
    let after = store.get("remote").unwrap().unwrap().tokens.unwrap();
    assert_ne!(before.access_token, after.access_token);
}

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("sion-mcp-http-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A browser: follows the authorization URL to its redirect, then to the
/// callback on this machine.
fn browse(url: &str) {
    let client = sion_http::Client::new(sion_http::Options {
        allow_private: true,
        proxy_from_env: false,
        ..sion_http::Options::default()
    })
    .unwrap();
    let response = client.send("GET", url, &[], None).unwrap();
    assert_eq!(response.status, 302, "{response:?}");
    let location = response.header("location").unwrap().to_string();
    let back = client.send("GET", &location, &[], None).unwrap();
    assert_eq!(back.status, 200);
}

#[test]
fn oauth_finds_registers_authorizes_and_renews() {
    let stand = stand_in(Era::Modern, true);
    let dir = temp("oauth");
    let store = AuthStore::at(&dir);
    let server = server(&stand);
    // Without a token, the server refuses, and says how to log in.
    let err = Connection::start(&server).unwrap_err();
    assert!(
        err.contains("needs authorization") && err.contains("sion agent mcp login remote"),
        "{err}"
    );

    let shown = Arc::new(Mutex::new(Vec::new()));
    let seen = shown.clone();
    let show = move |url: &str| {
        seen.lock().unwrap().push(url.to_string());
        let url = url.to_string();
        std::thread::spawn(move || browse(&url));
    };
    let lines =
        sion_cli::mcp_auth::login(&server, &OAuthConfig::default(), &store, None, &show).unwrap();
    let text = lines.join("\n");
    assert!(text.contains("registered with"), "{text}");
    assert!(text.contains("authorized remote at"), "{text}");
    let url = shown.lock().unwrap()[0].clone();
    let asked = oauth::query(url.split_once('?').unwrap().1);
    assert_eq!(asked["resource"], format!("{}/mcp", stand.base()));
    assert_eq!(asked["scope"], "tools:read");
    // What the login kept is encrypted.
    let raw = std::fs::read(dir.join("mcp-auth.bin")).unwrap();
    assert!(raw.starts_with(sion_cli::mcp_auth::MAGIC));
    assert!(!raw.windows(7).any(|w| w == b"access-"));

    let auth: Arc<dyn Authorizer> =
        Arc::new(Stored::new(&server, OAuthConfig::default(), store.clone()));
    let server = reviewed(server, Some(auth.clone()));
    let mounted = mcp::mount_with(&server, Some(auth.clone())).unwrap();
    assert_eq!(
        call(&mounted, "remote_echo", json!({"text": "authorized"})),
        "echo: authorized"
    );

    // The server stops taking the token: it is renewed with the refresh
    // token, which is rotated, and the call goes through.
    let before = store.get("remote").unwrap().unwrap().tokens.unwrap();
    stand.state.lock().unwrap().access.clear();
    assert_eq!(
        call(&mounted, "remote_echo", json!({"text": "again"})),
        "echo: again"
    );
    let after = store.get("remote").unwrap().unwrap().tokens.unwrap();
    assert_ne!(before.access_token, after.access_token);
    assert_ne!(before.refresh_token, after.refresh_token);
    assert!(!stand
        .state
        .lock()
        .unwrap()
        .refresh
        .contains(before.refresh_token.as_ref().unwrap()));

    // Logging out revokes the tokens at the authorization server, then
    // forgets them.
    drop(mounted);
    let lines = sion_cli::mcp_auth::logout(&server, &OAuthConfig::default(), &store)
        .unwrap()
        .unwrap();
    assert!(lines[0].starts_with("revoked its tokens at"), "{lines:?}");
    {
        let state = stand.state.lock().unwrap();
        assert!(!state
            .refresh
            .contains(after.refresh_token.as_ref().unwrap()));
        assert!(!state.access.contains(&after.access_token));
    }
    assert!(store.get("remote").unwrap().is_none());
    assert!(
        sion_cli::mcp_auth::logout(&server, &OAuthConfig::default(), &store)
            .unwrap()
            .is_none()
    );
}

#[test]
fn a_browser_on_another_machine_has_its_address_pasted() {
    let stand = stand_in(Era::Modern, true);
    let store = AuthStore::at(temp("pasted"));
    let server = server(&stand);
    let (paste, pasted) = std::sync::mpsc::channel();
    // The browser is elsewhere: it is sent back to this machine's loopback,
    // which it cannot reach, and the person pastes that address instead.
    let show = move |url: &str| {
        let client = sion_http::Client::new(sion_http::Options {
            allow_private: true,
            proxy_from_env: false,
            ..sion_http::Options::default()
        })
        .unwrap();
        let response = client.send("GET", url, &[], None).unwrap();
        let location = response.header("location").unwrap().to_string();
        paste.send("not the address".to_string()).unwrap();
        paste.send(String::new()).unwrap();
        paste.send(format!("  {location}  ")).unwrap();
    };
    let lines = sion_cli::mcp_auth::login_with(
        &server,
        &OAuthConfig::default(),
        &store,
        None,
        &show,
        Some(&pasted),
    )
    .unwrap();
    assert!(
        lines.iter().any(|l| l.contains("authorized remote at")),
        "{lines:?}"
    );
    let auth: Arc<dyn Authorizer> = Arc::new(Stored::new(&server, OAuthConfig::default(), store));
    let server = reviewed(server, Some(auth.clone()));
    let mounted = mcp::mount_with(&server, Some(auth)).unwrap();
    assert_eq!(
        call(&mounted, "remote_echo", json!({"text": "pasted"})),
        "echo: pasted"
    );
}

#[test]
fn a_client_of_its_own_uses_client_credentials() {
    let stand = stand_in(Era::Modern, true);
    let store = AuthStore::at(temp("robot"));
    std::env::set_var("SION_TEST_ROBOT_SECRET", "s3cret");
    let config = OAuthConfig {
        client_id: Some("robot".into()),
        client_secret_env: Some("SION_TEST_ROBOT_SECRET".into()),
        client_credentials: true,
        ..OAuthConfig::default()
    };
    let mut server = server(&stand);
    server.oauth = Some(config.clone());
    let lines = sion_cli::mcp_auth::login(&server, &config, &store, None, &|_| {
        panic!("no person is asked")
    })
    .unwrap();
    assert!(
        lines[0].contains("client credentials of robot"),
        "{lines:?}"
    );
    // The secret stays in its variable.
    assert!(store
        .get("remote")
        .unwrap()
        .unwrap()
        .registration
        .client_secret
        .is_none());
    let auth: Arc<dyn Authorizer> = Arc::new(Stored::new(&server, config, store));
    let server = reviewed(server, Some(auth.clone()));
    let mounted = mcp::mount_with(&server, Some(auth)).unwrap();
    // A token the server stops taking is replaced, with no person.
    stand.state.lock().unwrap().access.clear();
    assert_eq!(
        call(&mounted, "remote_echo", json!({"text": "robot"})),
        "echo: robot"
    );
}
