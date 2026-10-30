//! Authorization for MCP servers reached over HTTP: `sion agent mcp login`
//! and `logout`, and the [`Authorizer`] a session's runtime uses.
//!
//! What a login yields (the authorization server's metadata, the client's
//! registration with it, the tokens and the scopes asked for) is kept per
//! server in `agents/mcp-auth.bin`, encrypted like the secrets
//! (`crate::secrets`), never in `mcp.json`. A session sends the access
//! token, refreshes it when it is about to expire or the server refuses
//! it, and keeps the new tokens; one that cannot be had without a person
//! leaves the server's tools out, saying how to log in again.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sion_agent::mcp::Server;
use sion_agent::oauth::{self, Authorizer, Challenge, Fetcher};

use crate::error::CliError;

/// What the store's file starts with, and what its encryption is bound to.
pub const MAGIC: &[u8] = b"sion-agent-mcp-auth/v1\n";

/// The file in the store's directory.
pub const FILE: &str = "mcp-auth.bin";

/// How long a person has to authorize in their browser.
pub const LOGIN_SECS: u64 = 600;

pub use sion_agent::oauth::Config as OAuthConfig;

/// What a login left for one server.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServerAuth {
    /// The MCP server's canonical URI, the tokens' `resource`.
    pub resource: String,
    /// The authorization server's metadata.
    pub metadata: oauth::ServerMetadata,
    /// The client's registration with it.
    pub registration: oauth::Registration,
    /// The tokens, if any are held.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<oauth::Tokens>,
    /// The scopes asked for so far.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// Whether tokens come from the client credentials grant.
    #[serde(default)]
    pub client_credentials: bool,
}

/// Every server's authorization, by server name.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Store {
    /// By server name.
    pub servers: BTreeMap<String, ServerAuth>,
}

/// The store in `dir`.
#[derive(Clone, Debug)]
pub struct AuthStore {
    dir: std::path::PathBuf,
}

impl AuthStore {
    /// The store in `dir` (this machine's: [`crate::secrets::dir`]).
    pub fn at(dir: impl Into<std::path::PathBuf>) -> AuthStore {
        AuthStore { dir: dir.into() }
    }

    /// This machine's store.
    pub fn here() -> AuthStore {
        AuthStore::at(crate::secrets::dir())
    }

    /// Everything in it.
    pub fn load(&self) -> Result<Store, CliError> {
        crate::secrets::open_json(&self.dir, FILE, MAGIC)
    }

    /// Writes it, encrypted.
    pub fn save(&self, store: &Store) -> Result<(), CliError> {
        crate::secrets::seal_json(&self.dir, FILE, MAGIC, store)
    }

    /// The authorization of `server`, if there is one.
    pub fn get(&self, server: &str) -> Result<Option<ServerAuth>, CliError> {
        Ok(self.load()?.servers.remove(server))
    }

    /// Keeps `auth` for `server`.
    pub fn put(&self, server: &str, auth: ServerAuth) -> Result<(), CliError> {
        let mut store = self.load()?;
        store.servers.insert(server.to_string(), auth);
        self.save(&store)
    }

    /// Forgets `server`'s authorization. Returns whether there was one.
    pub fn remove(&self, server: &str) -> Result<bool, CliError> {
        let mut store = self.load()?;
        let removed = store.servers.remove(server).is_some();
        if removed {
            self.save(&store)?;
        }
        Ok(removed)
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn usage(e: impl std::fmt::Display) -> CliError {
    CliError::Usage(e.to_string())
}

/// The secret of a pre-registered client, from its environment variable.
fn secret_of(config: &OAuthConfig) -> Result<Option<String>, String> {
    match &config.client_secret_env {
        Some(var) => std::env::var(var)
            .map(Some)
            .map_err(|_| format!("the client secret's environment variable {var} is not set")),
        None => Ok(None),
    }
}

/// A session's authorization for one server, from the store.
#[derive(Debug)]
pub struct Stored {
    server: String,
    config: OAuthConfig,
    url: String,
    store: AuthStore,
    lock: Mutex<()>,
}

impl Stored {
    /// `server`'s authorization from `store`.
    pub fn new(server: &Server, config: OAuthConfig, store: AuthStore) -> Stored {
        Stored {
            server: server.name.clone(),
            config,
            url: server.url.clone().unwrap_or_default(),
            store,
            lock: Mutex::new(()),
        }
    }

    fn login_hint(&self) -> String {
        format!("log in with `sion agent mcp login {}`", self.server)
    }

    /// New tokens without a person: a refresh, or a new client
    /// credentials token (with `extra` scopes).
    fn renewed(&self, auth: &mut ServerAuth, extra: &[String]) -> Result<oauth::Tokens, String> {
        let fetch = Fetcher::for_server(&self.url)?;
        if auth.client_credentials {
            let mut registration = auth.registration.clone();
            if registration.client_secret.is_none() {
                registration.client_secret = secret_of(&self.config)?;
            }
            auth.scope = oauth::union(auth.scope.as_deref(), extra);
            return oauth::client_credentials(
                &fetch,
                &auth.metadata,
                &registration,
                &auth.resource,
                auth.scope.as_deref(),
            );
        }
        if !extra.is_empty() {
            return Err(format!(
                "the MCP server {} asks for more scopes ({}): {} --scope '{}'",
                self.server,
                extra.join(" "),
                self.login_hint(),
                extra.join(" ")
            ));
        }
        let refresh_token = auth
            .tokens
            .as_ref()
            .and_then(|t| t.refresh_token.clone())
            .ok_or_else(|| {
                format!(
                    "the MCP server {}'s authorization has run out: {}",
                    self.server,
                    self.login_hint()
                )
            })?;
        let mut registration = auth.registration.clone();
        if registration.client_secret.is_none() && registration.how == "pre-registered" {
            registration.client_secret = secret_of(&self.config)?;
        }
        oauth::refresh(
            &fetch,
            &auth.metadata,
            &registration,
            &refresh_token,
            &auth.resource,
        )
        .map_err(|e| format!("{e}: {}", self.login_hint()))
    }

    fn with_auth<T>(
        &self,
        f: impl FnOnce(&mut ServerAuth) -> Result<T, String>,
    ) -> Result<T, String> {
        let _guard = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut auth = self
            .store
            .get(&self.server)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| {
                format!(
                    "the MCP server {} has not been authorized: {}",
                    self.server,
                    self.login_hint()
                )
            })?;
        let out = f(&mut auth)?;
        self.store
            .put(&self.server, auth)
            .map_err(|e| e.to_string())?;
        Ok(out)
    }
}

impl Authorizer for Stored {
    fn header(&self) -> Result<Option<String>, String> {
        match self.store.get(&self.server).map_err(|e| e.to_string())? {
            None => Ok(None),
            Some(auth) => match &auth.tokens {
                Some(tokens) if tokens.fresh(now()) => Ok(Some(tokens.header())),
                _ => self.with_auth(|auth| {
                    let tokens = self.renewed(auth, &[])?;
                    let header = tokens.header();
                    auth.tokens = Some(tokens);
                    Ok(Some(header))
                }),
            },
        }
    }

    fn renew(&self, status: u16, challenges: &[Challenge]) -> Result<String, String> {
        let extra: Vec<String> = if status == 403 {
            challenges
                .iter()
                .filter(|c| c.scheme == "bearer")
                .flat_map(Challenge::scopes)
                .collect()
        } else {
            Vec::new()
        };
        self.with_auth(|auth| {
            let tokens = self.renewed(auth, &extra)?;
            let header = tokens.header();
            auth.tokens = Some(tokens);
            Ok(header)
        })
    }
}

/// The challenges an unauthorized request to `url` gets: a modern
/// `server/discover`, as a client would first send, and, when the URL
/// does not take it (`400`, `404`, `405`), the `GET` that opens the
/// deprecated HTTP+SSE transport's stream.
pub fn first_challenge(
    url: &str,
    headers: &[(String, String)],
) -> Result<Option<Vec<Challenge>>, String> {
    let client = sion_http::Client::new(sion_http::Options {
        allow_private: true,
        ..sion_http::Options::default()
    })
    .map_err(|e| e.to_string())?;
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "server/discover",
        "params": {"_meta": {
            "io.modelcontextprotocol/protocolVersion": sion_agent::mcp::MODERN,
            "io.modelcontextprotocol/clientInfo": {"name": "sion-agent", "version": env!("CARGO_PKG_VERSION")},
            "io.modelcontextprotocol/clientCapabilities": {}
        }}
    });
    let mut all: Vec<(&str, &str)> = vec![
        ("Content-Type", "application/json"),
        ("Accept", "application/json, text/event-stream"),
        ("MCP-Protocol-Version", sion_agent::mcp::MODERN),
        ("Mcp-Method", "server/discover"),
    ];
    all.extend(headers.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    let response = client
        .send("POST", url, &all, Some(body.to_string().as_bytes()))
        .map_err(|e| format!("{url}: {e}"))?;
    if response.status == 401 || response.status == 403 {
        return Ok(Some(oauth::challenges(response.all("www-authenticate"))));
    }
    if !matches!(response.status, 400 | 404 | 405) {
        return Ok(None);
    }
    drop(response);
    let mut get: Vec<(&str, &str)> = vec![("Accept", "text/event-stream")];
    get.extend(headers.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    // A stream that opens is let go at once.
    let response = client
        .send("GET", url, &get, None)
        .map_err(|e| format!("{url}: {e}"))?;
    Ok((response.status == 401 || response.status == 403)
        .then(|| oauth::challenges(response.all("www-authenticate"))))
}

/// What the person sees while a login waits for them: the URL to open.
pub type Show<'a> = &'a dyn Fn(&str);

/// Addresses a person pastes while a login waits: where a browser on
/// another machine was sent back to, which it could not reach.
pub type Pasted = std::sync::mpsc::Receiver<String>;

/// Logs in to `server` with `config`: discovers where and how to
/// authorize, registers if it must, has a person authorize in a browser
/// (or uses the client credentials), and keeps the tokens in `store`.
/// `extra_scope` adds scopes to ask for. `show` is given the URL to open.
pub fn login(
    server: &Server,
    config: &OAuthConfig,
    store: &AuthStore,
    extra_scope: Option<&str>,
    show: Show<'_>,
) -> Result<Vec<String>, CliError> {
    login_with(server, config, store, extra_scope, show, None)
}

/// [`login`], also taking the answer from an address the person pastes.
pub fn login_with(
    server: &Server,
    config: &OAuthConfig,
    store: &AuthStore,
    extra_scope: Option<&str>,
    show: Show<'_>,
    pasted: Option<&Pasted>,
) -> Result<Vec<String>, CliError> {
    let url = server.url.as_deref().ok_or_else(|| {
        usage(format!(
            "the MCP server {} is not reached over HTTP",
            server.name
        ))
    })?;
    let fetch = Fetcher::for_server(url).map_err(usage)?;
    let headers =
        sion_agent::mcp_http::configured_headers(&server.name, &server.headers).map_err(usage)?;
    let challenge = first_challenge(url, &headers).map_err(usage)?;
    let bearer = challenge
        .as_ref()
        .and_then(|found| found.iter().find(|c| c.scheme == "bearer").cloned());
    let mut lines = Vec::new();
    let resource_meta = oauth::protected_resource(
        &fetch,
        url,
        bearer.as_ref().and_then(|c| c.param("resource_metadata")),
    )
    .map_err(usage)?;
    let issuer = match &config.issuer {
        Some(issuer) if resource_meta.authorization_servers.contains(issuer) => issuer.clone(),
        Some(issuer) => {
            return Err(usage(format!(
                "the MCP server {} does not name {issuer} as an authorization server; it names {}",
                server.name,
                resource_meta.authorization_servers.join(", ")
            )))
        }
        None => resource_meta.authorization_servers[0].clone(),
    };
    let metadata = oauth::authorization_server(&fetch, &issuer).map_err(usage)?;
    let resource = oauth::canonical(url).map_err(usage)?;
    let previous = store.get(&server.name)?;
    // Scopes: what the server's challenge asks for (else what its metadata
    // lists), what was granted before, and what the operator adds.
    let asked: Vec<String> = match bearer.as_ref().map(Challenge::scopes) {
        Some(scopes) if !scopes.is_empty() => scopes,
        _ => resource_meta.scopes_supported.clone(),
    };
    let mut wanted: Vec<String> = asked;
    for extra in [config.scope.as_deref(), extra_scope].into_iter().flatten() {
        wanted.extend(extra.split_whitespace().map(str::to_string));
    }
    let scope = oauth::union(previous.as_ref().and_then(|p| p.scope.as_deref()), &wanted);
    let secret = secret_of(config).map_err(usage)?;
    // A registration is bound to the authorization server that issued it.
    let kept = previous
        .as_ref()
        .filter(|p| p.registration.issuer == metadata.issuer && p.registration.how == "dynamic")
        .map(|p| p.registration.clone());

    if config.client_credentials {
        let client_id = config.client_id.clone().ok_or_else(|| {
            usage("the client credentials grant needs --client-id and --client-secret-env")
        })?;
        let registration = oauth::Registration {
            issuer: metadata.issuer.clone(),
            client_id,
            client_secret: secret,
            redirect_uri: None,
            how: "pre-registered".into(),
        };
        let tokens = oauth::client_credentials(
            &fetch,
            &metadata,
            &registration,
            &resource,
            scope.as_deref(),
        )
        .map_err(usage)?;
        lines.push(format!(
            "authorized {} with the client credentials of {} at {}",
            server.name, registration.client_id, metadata.issuer
        ));
        store.put(
            &server.name,
            ServerAuth {
                resource,
                metadata,
                registration: oauth::Registration {
                    client_secret: None,
                    ..registration
                },
                tokens: Some(tokens),
                scope,
                client_credentials: true,
            },
        )?;
        return Ok(lines);
    }

    // The browser comes back to this machine.
    let wanted_port = config.callback_port.or_else(|| {
        kept.as_ref()
            .and_then(|r| r.redirect_uri.as_deref())
            .and_then(|uri| uri.rsplit_once(':'))
            .and_then(|(_, rest)| rest.split('/').next())
            .and_then(|port| port.parse().ok())
    });
    let listener = match tiny_http::Server::http(("127.0.0.1", wanted_port.unwrap_or(0))) {
        Ok(listener) => listener,
        Err(_) if config.callback_port.is_none() => tiny_http::Server::http(("127.0.0.1", 0))
            .map_err(|e| usage(format!("no port for the browser to come back to: {e}")))?,
        Err(e) => {
            return Err(usage(format!(
                "port {} is taken: the browser cannot come back to it ({e})",
                wanted_port.unwrap_or(0)
            )))
        }
    };
    let port = listener
        .server_addr()
        .to_ip()
        .map(|a| a.port())
        .ok_or_else(|| usage("the callback listener has no port"))?;
    let redirect_uri = format!("http://127.0.0.1:{port}{}", oauth::CALLBACK_PATH);

    let registration = if let Some(client_id) = &config.client_id {
        oauth::Registration {
            issuer: metadata.issuer.clone(),
            client_id: client_id.clone(),
            client_secret: secret.clone(),
            redirect_uri: Some(redirect_uri.clone()),
            how: "pre-registered".into(),
        }
    } else if let (Some(url), true) = (
        &config.client_metadata_url,
        metadata.client_id_metadata_document_supported,
    ) {
        oauth::Registration {
            issuer: metadata.issuer.clone(),
            client_id: url.clone(),
            client_secret: None,
            redirect_uri: Some(redirect_uri.clone()),
            how: "metadata-document".into(),
        }
    } else {
        match kept.filter(|r| r.redirect_uri.as_deref() == Some(redirect_uri.as_str())) {
            Some(kept) => kept,
            None => {
                let registration =
                    oauth::register(&fetch, &metadata, &redirect_uri, "Siontology agent (sion)")
                        .map_err(usage)?;
                lines.push(format!(
                    "registered with {} as client {}",
                    metadata.issuer, registration.client_id
                ));
                registration
            }
        }
    };
    let (authorize_url, pending) = oauth::begin(
        &metadata,
        &registration,
        &redirect_uri,
        &resource,
        scope.as_deref(),
    )
    .map_err(usage)?;
    show(&authorize_url);
    let answer = wait_for_callback(&listener, &pending, pasted).map_err(usage)?;
    let code = oauth::finish(&pending, &answer).map_err(usage)?;
    let tokens =
        oauth::exchange(&fetch, &metadata, &registration, &pending, &code).map_err(usage)?;
    lines.push(format!(
        "authorized {} at {}{}{}",
        server.name,
        metadata.issuer,
        tokens
            .scope
            .as_deref()
            .or(scope.as_deref())
            .map(|s| format!(" for {s}"))
            .unwrap_or_default(),
        if tokens.refresh_token.is_some() {
            ", renewed without you while its refresh token lasts"
        } else {
            ""
        }
    ));
    store.put(
        &server.name,
        ServerAuth {
            resource,
            metadata,
            registration: oauth::Registration {
                // A pre-registered client's secret stays in its variable.
                client_secret: if registration.how == "pre-registered" {
                    None
                } else {
                    registration.client_secret.clone()
                },
                ..registration
            },
            tokens: Some(tokens),
            scope,
            client_credentials: false,
        },
    )?;
    Ok(lines)
}

/// Waits for the browser to come back with the authorization's answer, or
/// for the person to paste the address it was sent back to, at most
/// [`LOGIN_SECS`]. Requests to other paths, and answers whose state is not
/// the one sent, are turned away and waited past.
fn wait_for_callback(
    listener: &tiny_http::Server,
    pending: &oauth::Pending,
    pasted: Option<&Pasted>,
) -> Result<BTreeMap<String, String>, String> {
    let deadline = std::time::Instant::now() + Duration::from_secs(LOGIN_SECS);
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return Err(format!(
                "no answer came back within {} minutes: run the login again",
                LOGIN_SECS / 60
            ));
        }
        if let Some(pasted) = pasted {
            while let Ok(line) = pasted.try_recv() {
                match answer_in(&line, pending) {
                    Ok(Some(answer)) => return Ok(answer),
                    Ok(None) => {}
                    Err(why) => eprintln!("{why}"),
                }
            }
        }
        let wait = match pasted {
            Some(_) => left.min(Duration::from_millis(200)),
            None => left,
        };
        let Ok(Some(request)) = listener.recv_timeout(wait) else {
            continue;
        };
        let target = request.url().to_string();
        let (path, query) = target.split_once('?').unwrap_or((&target, ""));
        if path != oauth::CALLBACK_PATH {
            let _ = request
                .respond(tiny_http::Response::from_string("not found").with_status_code(404));
            continue;
        }
        let answer = oauth::query(query);
        if answer.get("state") != Some(&pending.state) {
            let _ = request.respond(
                tiny_http::Response::from_string("This answer is not for the login under way.")
                    .with_status_code(400),
            );
            continue;
        }
        let page = if answer.contains_key("code") {
            "Authorized. You can close this tab and go back to the terminal."
        } else {
            "The authorization was not given. You can close this tab."
        };
        let _ = request.respond(
            tiny_http::Response::from_string(page).with_header(
                "Content-Type: text/plain; charset=utf-8"
                    .parse::<tiny_http::Header>()
                    .expect("a valid header"),
            ),
        );
        return Ok(answer);
    }
}

/// The authorization's answer in an address a person pasted: `None` for an
/// empty line.
fn answer_in(
    line: &str,
    pending: &oauth::Pending,
) -> Result<Option<BTreeMap<String, String>>, String> {
    let line = line.trim();
    if line.is_empty() {
        return Ok(None);
    }
    let query = line.split_once('?').map_or(line, |(_, q)| q);
    let query = query.split('#').next().unwrap_or(query);
    let answer = oauth::query(query);
    if answer.get("state") != Some(&pending.state)
        || !(answer.contains_key("code") || answer.contains_key("error"))
    {
        return Err(
            "that is not the answer to this login: paste the whole address the browser was sent back to"
                .into(),
        );
    }
    Ok(Some(answer))
}

/// Forgets `server`'s authorization. Its tokens are revoked first at the
/// authorization server, where that names a revocation endpoint (RFC
/// 7009); either way they are removed from `store`. Returns what was done,
/// or `None` when there was no authorization to forget.
pub fn logout(
    server: &Server,
    config: &OAuthConfig,
    store: &AuthStore,
) -> Result<Option<Vec<String>>, CliError> {
    let Some(auth) = store.get(&server.name)? else {
        return Ok(None);
    };
    let mut lines = Vec::new();
    if let Some(tokens) = &auth.tokens {
        let issuer = &auth.metadata.issuer;
        let mut registration = auth.registration.clone();
        if registration.client_secret.is_none() && registration.how == "pre-registered" {
            registration.client_secret = secret_of(config).ok().flatten();
        }
        let revoked = Fetcher::for_server(&auth.resource).and_then(|fetch| {
            let mut any = false;
            for (token, hint) in [
                (tokens.refresh_token.as_deref(), "refresh_token"),
                (Some(tokens.access_token.as_str()), "access_token"),
            ] {
                if let Some(token) = token {
                    if !oauth::revoke(&fetch, &auth.metadata, &registration, token, hint)? {
                        return Ok(false);
                    }
                    any = true;
                }
            }
            Ok(any)
        });
        lines.push(match revoked {
            Ok(true) => format!("revoked its tokens at {issuer}"),
            Ok(false) => format!(
                "{issuer} names no revocation endpoint: its tokens stay valid there until they expire"
            ),
            Err(why) => {
                format!("could not revoke its tokens ({why}): they stay valid there until they expire")
            }
        });
    }
    store.remove(&server.name)?;
    lines.push(format!(
        "forgot the authorization of MCP server {}",
        server.name
    ));
    Ok(Some(lines))
}

/// Opens `url` in the person's browser, if this machine has one to open.
pub fn open_browser(url: &str) -> bool {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "explorer"
    } else {
        "xdg-open"
    };
    std::process::Command::new(program)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// A summary of `server`'s authorization, for `sion agent mcp list`.
pub fn describe(auth: &ServerAuth) -> String {
    let state = match &auth.tokens {
        Some(tokens) if tokens.fresh(now()) => "authorized".to_string(),
        Some(tokens) if tokens.refresh_token.is_some() || auth.client_credentials => {
            "authorized (its access token will be renewed)".to_string()
        }
        Some(_) => "expired: log in again".to_string(),
        None => "not authorized".to_string(),
    };
    format!(
        "{state} at {}{}",
        auth.metadata.issuer,
        auth.scope
            .as_deref()
            .map(|s| format!(" for {s}"))
            .unwrap_or_default()
    )
}

/// The value of `x` as JSON, for a debugging dump without its secrets.
pub fn redacted(auth: &ServerAuth) -> Value {
    json!({
        "resource": auth.resource,
        "issuer": auth.metadata.issuer,
        "client_id": auth.registration.client_id,
        "registered": auth.registration.how,
        "scope": auth.scope,
        "expires_at": auth.tokens.as_ref().and_then(|t| t.expires_at),
        "refreshable": auth.tokens.as_ref().is_some_and(|t| t.refresh_token.is_some()),
    })
}
