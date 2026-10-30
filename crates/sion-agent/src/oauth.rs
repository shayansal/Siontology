//! OAuth 2.1 for MCP servers reached over HTTP, as the MCP authorization
//! specification (2025-06-18 on, 2026-07-28 included) has a client do it.
//!
//! A server that wants a token answers `401` with a `WWW-Authenticate`
//! challenge. The client finds the server's protected resource metadata
//! (RFC 9728: the challenge's `resource_metadata`, else the well-known
//! URIs, path first), which names its authorization servers; finds the
//! chosen one's metadata (RFC 8414, then OpenID Connect discovery, in the
//! order the specification gives) and keeps it only if its `issuer` is the
//! one asked for and it offers PKCE with `S256`; registers (a client the
//! operator registered, a client ID metadata document, or dynamic client
//! registration, RFC 7591, in that order of preference); and sends a person
//! to the authorization endpoint with PKCE, a `state`, the scopes the
//! challenge asked for and the server's canonical URI as `resource` (RFC
//! 8707). The answer is checked before its code is used: its `iss` against
//! the issuer recorded when it started (RFC 9207), then its `state`. Tokens
//! are refreshed without a person while a refresh token lasts. A client of
//! its own (an agent running unattended) can instead use the client
//! credentials grant.
//!
//! Every authorization server endpoint must be HTTPS, and every URL fetched
//! is held to public addresses, unless the MCP server is on this machine
//! (`local`), for development and tests.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// The path of the loopback redirect URI a person's browser comes back to.
pub const CALLBACK_PATH: &str = "/callback";

/// The most bytes of a metadata document or a token response.
const MAX_DOCUMENT: u64 = 256 << 10;

/// Seconds before its expiry that an access token is renewed.
pub const RENEW_MARGIN_SECS: u64 = 60;

/// How an MCP server over HTTP is authorized with OAuth, as its operator
/// configured it. What a login yields (registration, tokens) is kept apart,
/// encrypted, by the runtime.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    /// A client id registered with the authorization server, if the
    /// operator has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// The environment variable holding that client's secret, if it has
    /// one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret_env: Option<String>,
    /// The URL of this client's metadata document, used as its client id
    /// where the authorization server takes one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_metadata_url: Option<String>,
    /// The loopback port the browser comes back to (default: any free one).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callback_port: Option<u16>,
    /// The authorization server to use, when the server names several.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issuer: Option<String>,
    /// Scopes to ask for besides those the server asks for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// Whether the client acts on its own behalf (the client credentials
    /// grant), with no person.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub client_credentials: bool,
}

/// A `WWW-Authenticate` challenge: its scheme and parameters.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Challenge {
    /// The scheme, lower-cased (`bearer`).
    pub scheme: String,
    /// Its parameters, names lower-cased.
    pub params: BTreeMap<String, String>,
}

impl Challenge {
    /// The parameter `name`.
    pub fn param(&self, name: &str) -> Option<&str> {
        self.params.get(name).map(String::as_str)
    }

    /// The scopes it asks for.
    pub fn scopes(&self) -> Vec<String> {
        self.param("scope")
            .map(|s| s.split_whitespace().map(str::to_string).collect())
            .unwrap_or_default()
    }
}

/// The challenges of one or more `WWW-Authenticate` header values:
/// `Bearer resource_metadata="https://…", scope="files:read"`, several
/// schemes in one value, or several values.
pub fn challenges<'a>(values: impl IntoIterator<Item = &'a str>) -> Vec<Challenge> {
    let mut out: Vec<Challenge> = Vec::new();
    for value in values {
        let chars: Vec<char> = value.chars().collect();
        let mut i = 0;
        let skip = |i: &mut usize, also_commas: bool| {
            while *i < chars.len()
                && (chars[*i].is_whitespace() || (also_commas && chars[*i] == ','))
            {
                *i += 1;
            }
        };
        loop {
            skip(&mut i, true);
            if i >= chars.len() {
                break;
            }
            let start = i;
            while i < chars.len() && !chars[i].is_whitespace() && chars[i] != ',' && chars[i] != '='
            {
                i += 1;
            }
            let token: String = chars[start..i].iter().collect();
            let mut j = i;
            skip(&mut j, false);
            if j < chars.len() && chars[j] == '=' && !out.is_empty() {
                // A parameter of the challenge being read.
                j += 1;
                skip(&mut j, false);
                let mut value = String::new();
                if j < chars.len() && chars[j] == '"' {
                    j += 1;
                    while j < chars.len() && chars[j] != '"' {
                        if chars[j] == '\\' && j + 1 < chars.len() {
                            j += 1;
                        }
                        value.push(chars[j]);
                        j += 1;
                    }
                    j += 1;
                } else {
                    while j < chars.len() && chars[j] != ',' && !chars[j].is_whitespace() {
                        value.push(chars[j]);
                        j += 1;
                    }
                }
                if let Some(last) = out.last_mut() {
                    last.params.insert(token.to_ascii_lowercase(), value);
                }
                i = j;
            } else if !token.is_empty() {
                out.push(Challenge {
                    scheme: token.to_ascii_lowercase(),
                    params: BTreeMap::new(),
                });
            } else {
                i += 1;
            }
        }
    }
    out
}

/// A server's protected resource metadata (RFC 9728), as this client uses
/// it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resource {
    /// The resource identifier.
    pub resource: String,
    /// Its authorization servers' issuers.
    pub authorization_servers: Vec<String>,
    /// The scopes it supports, if it says.
    #[serde(default)]
    pub scopes_supported: Vec<String>,
}

/// An authorization server's metadata (RFC 8414, or OpenID Connect
/// discovery), as this client uses it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerMetadata {
    /// Its issuer identifier.
    pub issuer: String,
    /// Where a person authorizes.
    #[serde(default)]
    pub authorization_endpoint: Option<String>,
    /// Where tokens are issued.
    pub token_endpoint: String,
    /// Where clients register themselves, if it lets them.
    #[serde(default)]
    pub registration_endpoint: Option<String>,
    /// The scopes it supports, if it says.
    #[serde(default)]
    pub scopes_supported: Vec<String>,
    /// The PKCE methods it supports.
    #[serde(default)]
    pub code_challenge_methods_supported: Vec<String>,
    /// The grant types it supports, if it says.
    #[serde(default)]
    pub grant_types_supported: Vec<String>,
    /// How clients authenticate at its token endpoint, if it says.
    #[serde(default)]
    pub token_endpoint_auth_methods_supported: Vec<String>,
    /// Whether it takes a client ID metadata document's URL as a client id.
    #[serde(default)]
    pub client_id_metadata_document_supported: bool,
    /// Whether its authorization responses carry `iss` (RFC 9207).
    #[serde(default)]
    pub authorization_response_iss_parameter_supported: bool,
    /// Where tokens are revoked (RFC 7009), if it says.
    #[serde(default)]
    pub revocation_endpoint: Option<String>,
    /// How clients authenticate at its revocation endpoint, if it says.
    #[serde(default)]
    pub revocation_endpoint_auth_methods_supported: Vec<String>,
}

/// Fetches what discovery and the token endpoint need, holding them to
/// HTTPS and public addresses unless the MCP server is on this machine
/// (then plain HTTP there too) or at a private address itself (then the
/// private addresses of its network too).
#[derive(Clone, Debug)]
pub struct Fetcher {
    client: sion_http::Client,
    local: bool,
}

/// The origin (`scheme://authority`) and the path of a URL.
fn split_url(url: &str) -> Result<(String, String), String> {
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| format!("{url:?} is not an absolute URL"))?;
    let rest = rest.split('#').next().unwrap_or_default();
    let cut = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(cut);
    if authority.is_empty() {
        return Err(format!("{url:?} has no host"));
    }
    let path = tail.split('?').next().unwrap_or_default();
    Ok((
        format!(
            "{}://{}",
            scheme.to_ascii_lowercase(),
            authority.to_ascii_lowercase()
        ),
        path.to_string(),
    ))
}

/// Whether `url` is on this machine (`localhost`, `127.0.0.1`, `[::1]`).
pub fn is_loopback(url: &str) -> bool {
    let Ok((origin, _)) = split_url(url) else {
        return false;
    };
    let authority = origin.split("://").nth(1).unwrap_or_default();
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or_default().to_string(),
        None => authority.split(':').next().unwrap_or_default().to_string(),
    };
    host == "localhost"
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// The canonical URI of an MCP server, its `resource` (RFC 8707): scheme
/// and host lower-cased, no fragment, no query, no trailing slash.
pub fn canonical(url: &str) -> Result<String, String> {
    let (origin, path) = split_url(url)?;
    Ok(format!("{origin}{}", path.trim_end_matches('/')))
}

/// Whether the resource identifier `resource` covers the MCP server at
/// `server`: the same origin, and the server's path at or below its path.
pub fn covers(resource: &str, server: &str) -> bool {
    match (split_url(resource), split_url(server)) {
        (Ok((a, pa)), Ok((b, pb))) => {
            let pa = pa.trim_end_matches('/');
            let pb = pb.trim_end_matches('/');
            a == b && (pa.is_empty() || pb == pa || pb.starts_with(&format!("{pa}/")))
        }
        _ => false,
    }
}

/// Percent-encodes `text` for a URL's query or a form (RFC 3986's
/// unreserved characters are kept).
pub fn encode(text: &str) -> String {
    let mut out = String::new();
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Decodes a percent-encoded query or form value (`+` is a space).
pub fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'+', _) => out.push(b' '),
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 2;
            }
            (byte, _) => out.push(byte),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The parameters of a query string (`a=1&b=2`).
pub fn query(text: &str) -> BTreeMap<String, String> {
    text.split('&')
        .filter(|p| !p.is_empty())
        .map(|p| match p.split_once('=') {
            Some((k, v)) => (decode(k), decode(v)),
            None => (decode(p), String::new()),
        })
        .collect()
}

fn form(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Whether the host of `url` is at private addresses only (a local
/// network's, this machine's), as it resolves here.
pub fn is_private(url: &str) -> bool {
    use std::net::ToSocketAddrs;
    let Ok((origin, _)) = split_url(url) else {
        return false;
    };
    let (scheme, authority) = origin.split_once("://").unwrap_or_default();
    let (host, port) = match authority.strip_prefix('[') {
        Some(v6) => {
            let (host, rest) = v6.split_once(']').unwrap_or((v6, ""));
            (
                host.to_string(),
                rest.strip_prefix(':').and_then(|p| p.parse().ok()),
            )
        }
        None => match authority.rsplit_once(':') {
            Some((host, port)) => (host.to_string(), port.parse().ok()),
            None => (authority.to_string(), None),
        },
    };
    let port: u16 = port.unwrap_or(if scheme == "https" { 443 } else { 80 });
    match (host.as_str(), port).to_socket_addrs() {
        Ok(addresses) => {
            let addresses: Vec<_> = addresses.collect();
            !addresses.is_empty() && addresses.iter().all(|a| !sion_http::is_public(a.ip()))
        }
        Err(_) => false,
    }
}

impl Fetcher {
    /// A fetcher; `local` lets it reach this machine and plain HTTP there.
    pub fn new(local: bool) -> Result<Fetcher, String> {
        Fetcher::with(local, local)
    }

    /// The fetcher for the MCP server at `url`: as [`Fetcher`] says, by
    /// where the server is.
    pub fn for_server(url: &str) -> Result<Fetcher, String> {
        let local = is_loopback(url);
        Fetcher::with(local, local || is_private(url))
    }

    fn with(local: bool, private: bool) -> Result<Fetcher, String> {
        let client = sion_http::Client::new(sion_http::Options {
            allow_private: private,
            timeout: std::time::Duration::from_secs(30),
            ..sion_http::Options::default()
        })
        .map_err(|e| e.to_string())?;
        Ok(Fetcher { client, local })
    }

    /// Whether `url` may be used: HTTPS, or HTTP on this machine when
    /// local.
    pub fn check(&self, url: &str) -> Result<(), String> {
        if url.starts_with("https://")
            || (self.local && url.starts_with("http://") && is_loopback(url))
        {
            Ok(())
        } else {
            Err(format!(
                "{url} is not an HTTPS URL: authorization servers and their metadata must be served over HTTPS"
            ))
        }
    }

    fn json(&self, response: sion_http::Response) -> Result<(u16, Value), String> {
        let status = response.status;
        let body = response.read_all(MAX_DOCUMENT).map_err(|e| e.to_string())?;
        let value = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body).unwrap_or(Value::Null)
        };
        Ok((status, value))
    }

    /// `GET url`, as JSON.
    pub fn get_json(&self, url: &str) -> Result<(u16, Value), String> {
        self.check(url)?;
        let response = self
            .client
            .send("GET", url, &[("Accept", "application/json")], None)
            .map_err(|e| format!("{url}: {e}"))?;
        self.json(response)
    }

    /// `POST url` with a form, and HTTP Basic credentials if given.
    pub fn post_form(
        &self,
        url: &str,
        pairs: &[(&str, &str)],
        basic: Option<(&str, &str)>,
    ) -> Result<(u16, Value), String> {
        self.check(url)?;
        let auth = basic.map(|(id, secret)| {
            format!(
                "Basic {}",
                crate::encoding::base64(format!("{}:{}", encode(id), encode(secret)).as_bytes())
            )
        });
        let mut headers = vec![
            ("Content-Type", "application/x-www-form-urlencoded"),
            ("Accept", "application/json"),
        ];
        if let Some(auth) = &auth {
            headers.push(("Authorization", auth.as_str()));
        }
        let response = self
            .client
            .send("POST", url, &headers, Some(form(pairs).as_bytes()))
            .map_err(|e| format!("{url}: {e}"))?;
        self.json(response)
    }

    /// `POST url` with a JSON body.
    pub fn post_json(&self, url: &str, body: &Value) -> Result<(u16, Value), String> {
        self.check(url)?;
        let response = self
            .client
            .send(
                "POST",
                url,
                &[
                    ("Content-Type", "application/json"),
                    ("Accept", "application/json"),
                ],
                Some(body.to_string().as_bytes()),
            )
            .map_err(|e| format!("{url}: {e}"))?;
        self.json(response)
    }
}

/// The protected resource metadata of the MCP server at `server`: from the
/// challenge's `resource_metadata` URL if it gave one, else from the
/// well-known URI with the server's path, else at the root. It must name
/// the server (the same origin, the server's path at or below its own)
/// and at least one authorization server.
pub fn protected_resource(
    fetch: &Fetcher,
    server: &str,
    from_challenge: Option<&str>,
) -> Result<Resource, String> {
    let (origin, path) = split_url(server)?;
    let path = path.trim_end_matches('/');
    let mut candidates = Vec::new();
    if let Some(url) = from_challenge {
        candidates.push(url.to_string());
    } else {
        if !path.is_empty() {
            candidates.push(format!(
                "{origin}/.well-known/oauth-protected-resource{path}"
            ));
        }
        candidates.push(format!("{origin}/.well-known/oauth-protected-resource"));
    }
    let mut tried = Vec::new();
    for url in candidates {
        let (status, value) = match fetch.get_json(&url) {
            Ok(answer) => answer,
            Err(e) => {
                tried.push(e);
                continue;
            }
        };
        if status != 200 || !value.is_object() {
            tried.push(format!("{url}: HTTP {status}"));
            continue;
        }
        let resource: Resource = serde_json::from_value(value)
            .map_err(|e| format!("{url} is not protected resource metadata: {e}"))?;
        if !covers(&resource.resource, server) {
            return Err(format!(
                "{url} describes {}, not the MCP server {server}",
                resource.resource
            ));
        }
        if resource.authorization_servers.is_empty() {
            return Err(format!("{url} names no authorization server"));
        }
        return Ok(resource);
    }
    Err(format!(
        "the MCP server {server} says where to authorize nowhere this client can read ({})",
        tried.join("; ")
    ))
}

/// The metadata of the authorization server `issuer`, from the first of
/// the well-known URIs the MCP specification lists that answers, kept
/// only if its `issuer` is `issuer` exactly and it supports PKCE with
/// `S256`.
pub fn authorization_server(fetch: &Fetcher, issuer: &str) -> Result<ServerMetadata, String> {
    fetch.check(issuer)?;
    let (origin, path) = split_url(issuer)?;
    let path = path.trim_end_matches('/');
    let candidates = if path.is_empty() {
        vec![
            format!("{origin}/.well-known/oauth-authorization-server"),
            format!("{origin}/.well-known/openid-configuration"),
        ]
    } else {
        vec![
            format!("{origin}/.well-known/oauth-authorization-server{path}"),
            format!("{origin}/.well-known/openid-configuration{path}"),
            format!("{origin}{path}/.well-known/openid-configuration"),
        ]
    };
    let mut tried = Vec::new();
    for url in candidates {
        let (status, value) = match fetch.get_json(&url) {
            Ok(answer) => answer,
            Err(e) => {
                tried.push(e);
                continue;
            }
        };
        if status != 200 || !value.is_object() {
            tried.push(format!("{url}: HTTP {status}"));
            continue;
        }
        let meta: ServerMetadata = serde_json::from_value(value)
            .map_err(|e| format!("{url} is not authorization server metadata: {e}"))?;
        if meta.issuer != issuer {
            return Err(format!(
                "{url} says its issuer is {}, not {issuer}: its metadata is not used",
                meta.issuer
            ));
        }
        if !meta
            .code_challenge_methods_supported
            .iter()
            .any(|m| m == "S256")
        {
            return Err(format!(
                "the authorization server {issuer} does not offer PKCE with S256, which this client requires"
            ));
        }
        fetch.check(&meta.token_endpoint)?;
        if let Some(endpoint) = &meta.authorization_endpoint {
            fetch.check(endpoint)?;
        }
        if let Some(endpoint) = &meta.registration_endpoint {
            fetch.check(endpoint)?;
        }
        return Ok(meta);
    }
    Err(format!(
        "the authorization server {issuer} has no metadata this client can read ({})",
        tried.join("; ")
    ))
}

/// A client registered with an authorization server.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registration {
    /// The authorization server it is registered with (its issuer).
    pub issuer: String,
    /// Its client id.
    pub client_id: String,
    /// Its secret, for a confidential client.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    /// The redirect URI it registered, if it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirect_uri: Option<String>,
    /// How it was registered: `pre-registered`, `metadata-document` or
    /// `dynamic`.
    pub how: String,
}

impl std::fmt::Debug for Registration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registration")
            .field("issuer", &self.issuer)
            .field("client_id", &self.client_id)
            .field("client_secret", &self.client_secret.as_ref().map(|_| "…"))
            .field("redirect_uri", &self.redirect_uri)
            .field("how", &self.how)
            .finish()
    }
}

/// Registers a public client for `redirect_uri` with dynamic client
/// registration (RFC 7591), as a native application.
pub fn register(
    fetch: &Fetcher,
    meta: &ServerMetadata,
    redirect_uri: &str,
    client_name: &str,
) -> Result<Registration, String> {
    let endpoint = meta.registration_endpoint.as_ref().ok_or_else(|| {
        format!(
            "the authorization server {} takes no registrations: give a client id registered with it",
            meta.issuer
        )
    })?;
    let (status, value) = fetch.post_json(
        endpoint,
        &json!({
            "client_name": client_name,
            "redirect_uris": [redirect_uri],
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
            "application_type": "native",
        }),
    )?;
    if !(200..300).contains(&status) {
        return Err(format!(
            "the authorization server {} refused to register this client (HTTP {status}): {}",
            meta.issuer,
            describe_error(&value)
        ));
    }
    let client_id = value["client_id"]
        .as_str()
        .filter(|id| !id.is_empty())
        .ok_or("the registration gave no client id")?;
    Ok(Registration {
        issuer: meta.issuer.clone(),
        client_id: client_id.to_string(),
        client_secret: value["client_secret"].as_str().map(str::to_string),
        redirect_uri: Some(redirect_uri.to_string()),
        how: "dynamic".into(),
    })
}

fn describe_error(value: &Value) -> String {
    match (value["error"].as_str(), value["error_description"].as_str()) {
        (Some(e), Some(d)) => format!("{e}: {d}"),
        (Some(e), None) => e.to_string(),
        _ => "no reason given".to_string(),
    }
}

fn random_text(bytes: usize) -> String {
    let mut buffer = vec![0u8; bytes];
    sion_http::random(&mut buffer).expect("the system gives random bytes");
    crate::encoding::base64_url(&buffer)
}

/// The PKCE challenge (`S256`) of a verifier.
pub fn challenge_of(verifier: &str) -> String {
    crate::encoding::base64_url(&sion_http::sha256(verifier.as_bytes()))
}

/// An authorization a person was sent to make: what its answer is checked
/// against, and what the token request needs.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pending {
    /// The `state` sent.
    pub state: String,
    /// The PKCE verifier.
    pub verifier: String,
    /// The issuer recorded before the person was sent away.
    pub issuer: String,
    /// Whether the answer must carry `iss` (RFC 9207).
    pub iss_required: bool,
    /// The redirect URI.
    pub redirect_uri: String,
    /// The `resource`: the MCP server's canonical URI.
    pub resource: String,
    /// The scopes asked for.
    #[serde(default)]
    pub scope: Option<String>,
}

impl std::fmt::Debug for Pending {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pending")
            .field("issuer", &self.issuer)
            .field("redirect_uri", &self.redirect_uri)
            .field("resource", &self.resource)
            .field("scope", &self.scope)
            .finish_non_exhaustive()
    }
}

/// The URL to send a person to, to authorize `registration` for
/// `resource` with `scope`, and what its answer will be checked against.
pub fn begin(
    meta: &ServerMetadata,
    registration: &Registration,
    redirect_uri: &str,
    resource: &str,
    scope: Option<&str>,
) -> Result<(String, Pending), String> {
    let endpoint = meta.authorization_endpoint.as_ref().ok_or_else(|| {
        format!(
            "the authorization server {} has no authorization endpoint",
            meta.issuer
        )
    })?;
    let pending = Pending {
        state: random_text(24),
        verifier: random_text(32),
        issuer: meta.issuer.clone(),
        iss_required: meta.authorization_response_iss_parameter_supported,
        redirect_uri: redirect_uri.to_string(),
        resource: resource.to_string(),
        scope: scope.map(str::to_string).filter(|s| !s.is_empty()),
    };
    let challenge = challenge_of(&pending.verifier);
    let mut pairs = vec![
        ("response_type", "code"),
        ("client_id", registration.client_id.as_str()),
        ("redirect_uri", redirect_uri),
        ("code_challenge", challenge.as_str()),
        ("code_challenge_method", "S256"),
        ("state", pending.state.as_str()),
        ("resource", resource),
    ];
    if let Some(scope) = &pending.scope {
        pairs.push(("scope", scope.as_str()));
    }
    let separator = if endpoint.contains('?') { '&' } else { '?' };
    Ok((format!("{endpoint}{separator}{}", form(&pairs)), pending))
}

/// The code of an authorization's answer (the query of the redirect),
/// checked first for its issuer (RFC 9207: `iss` must be the recorded
/// issuer when present, and present when the server says it sends it),
/// then for its `state`. An error answer from the wrong issuer is not
/// even shown.
pub fn finish(pending: &Pending, answer: &BTreeMap<String, String>) -> Result<String, String> {
    match answer.get("iss") {
        Some(iss) if *iss != pending.issuer => {
            return Err(format!(
                "the answer comes from issuer {iss}, not {}: it is not used",
                pending.issuer
            ))
        }
        None if pending.iss_required => {
            return Err(format!(
                "the answer does not say it comes from {}, as that server says its answers do: it is not used",
                pending.issuer
            ))
        }
        _ => {}
    }
    if answer.get("state") != Some(&pending.state) {
        return Err("the answer's state is not the one sent: it is not used".into());
    }
    if let Some(error) = answer.get("error") {
        return Err(format!(
            "the authorization was not given: {error}{}",
            answer
                .get("error_description")
                .map(|d| format!(" ({d})"))
                .unwrap_or_default()
        ));
    }
    answer
        .get("code")
        .filter(|c| !c.is_empty())
        .cloned()
        .ok_or_else(|| "the answer carries no code".to_string())
}

/// Tokens for an MCP server.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tokens {
    /// The access token.
    pub access_token: String,
    /// Its type (`Bearer`).
    pub token_type: String,
    /// The refresh token, if one was issued.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// When the access token expires (seconds since the epoch), if said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
    /// The scopes granted, if said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

impl std::fmt::Debug for Tokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tokens")
            .field("token_type", &self.token_type)
            .field("refresh_token", &self.refresh_token.as_ref().map(|_| "…"))
            .field("expires_at", &self.expires_at)
            .field("scope", &self.scope)
            .finish_non_exhaustive()
    }
}

impl Tokens {
    /// Whether the access token can still be used at `now` (seconds),
    /// with [`RENEW_MARGIN_SECS`] to spare.
    pub fn fresh(&self, now: u64) -> bool {
        self.expires_at
            .map_or(true, |at| now.saturating_add(RENEW_MARGIN_SECS) < at)
    }

    /// The `Authorization` header value.
    pub fn header(&self) -> String {
        format!("Bearer {}", self.access_token)
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// How the client authenticates at an endpoint that takes `methods`: HTTP
/// Basic with a secret when it takes that (or says nothing), the secret in
/// the form when it takes only that, the client id alone for a public
/// client. Returns the Basic credentials, if those.
fn client_auth<'a>(
    registration: &'a Registration,
    methods: &[String],
    pairs: &mut Vec<(&'a str, &'a str)>,
) -> Option<(&'a str, &'a str)> {
    match &registration.client_secret {
        Some(secret)
            if methods.is_empty() || methods.iter().any(|m| m == "client_secret_basic") =>
        {
            Some((registration.client_id.as_str(), secret.as_str()))
        }
        Some(secret) => {
            pairs.push(("client_id", registration.client_id.as_str()));
            pairs.push(("client_secret", secret.as_str()));
            None
        }
        None => {
            pairs.push(("client_id", registration.client_id.as_str()));
            None
        }
    }
}

/// A token request, the client authenticated as [`client_auth`] says, and
/// the answer read.
fn token_request<'a>(
    fetch: &Fetcher,
    meta: &ServerMetadata,
    registration: &'a Registration,
    mut pairs: Vec<(&'a str, &'a str)>,
    previous_refresh: Option<&str>,
) -> Result<Tokens, String> {
    let basic = client_auth(
        registration,
        &meta.token_endpoint_auth_methods_supported,
        &mut pairs,
    );
    let (status, value) = fetch.post_form(&meta.token_endpoint, &pairs, basic)?;
    if status != 200 {
        return Err(format!(
            "the authorization server {} issued no token (HTTP {status}): {}",
            meta.issuer,
            describe_error(&value)
        ));
    }
    let access_token = value["access_token"]
        .as_str()
        .filter(|t| !t.is_empty())
        .ok_or("the token response has no access token")?;
    let token_type = value["token_type"].as_str().unwrap_or("Bearer");
    if !token_type.eq_ignore_ascii_case("bearer") {
        return Err(format!(
            "the authorization server issued a {token_type} token; this client sends bearer tokens only"
        ));
    }
    Ok(Tokens {
        access_token: access_token.to_string(),
        token_type: "Bearer".into(),
        // A server that does not rotate refresh tokens keeps the old one.
        refresh_token: value["refresh_token"]
            .as_str()
            .map(str::to_string)
            .or_else(|| previous_refresh.map(str::to_string)),
        expires_at: value["expires_in"]
            .as_u64()
            .map(|s| now().saturating_add(s)),
        scope: value["scope"].as_str().map(str::to_string),
    })
}

/// Exchanges an authorization's code for tokens, with its PKCE verifier
/// and `resource`.
pub fn exchange(
    fetch: &Fetcher,
    meta: &ServerMetadata,
    registration: &Registration,
    pending: &Pending,
    code: &str,
) -> Result<Tokens, String> {
    let pairs = vec![
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", pending.redirect_uri.as_str()),
        ("code_verifier", pending.verifier.as_str()),
        ("resource", pending.resource.as_str()),
    ];
    token_request(fetch, meta, registration, pairs, None)
}

/// New tokens from a refresh token, for `resource`.
pub fn refresh(
    fetch: &Fetcher,
    meta: &ServerMetadata,
    registration: &Registration,
    refresh_token: &str,
    resource: &str,
) -> Result<Tokens, String> {
    let pairs = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("resource", resource),
    ];
    token_request(fetch, meta, registration, pairs, Some(refresh_token))
}

/// Tokens for a client acting on its own behalf (the client credentials
/// grant), for `resource`.
pub fn client_credentials(
    fetch: &Fetcher,
    meta: &ServerMetadata,
    registration: &Registration,
    resource: &str,
    scope: Option<&str>,
) -> Result<Tokens, String> {
    if registration.client_secret.is_none() {
        return Err("the client credentials grant needs the client's secret".into());
    }
    let mut pairs = vec![("grant_type", "client_credentials"), ("resource", resource)];
    if let Some(scope) = scope.filter(|s| !s.is_empty()) {
        pairs.push(("scope", scope));
    }
    token_request(fetch, meta, registration, pairs, None)
}

/// Revokes `token` at the authorization server (RFC 7009), `hint` saying
/// what it is (`refresh_token` or `access_token`). `Ok(false)` when the
/// server names no revocation endpoint. A server that does not know the
/// token answers as if it had revoked it.
pub fn revoke(
    fetch: &Fetcher,
    meta: &ServerMetadata,
    registration: &Registration,
    token: &str,
    hint: &str,
) -> Result<bool, String> {
    let Some(endpoint) = &meta.revocation_endpoint else {
        return Ok(false);
    };
    let methods = if meta.revocation_endpoint_auth_methods_supported.is_empty() {
        &meta.token_endpoint_auth_methods_supported
    } else {
        &meta.revocation_endpoint_auth_methods_supported
    };
    let mut pairs = vec![("token", token), ("token_type_hint", hint)];
    let basic = client_auth(registration, methods, &mut pairs);
    let (mut status, mut value) = fetch.post_form(endpoint, &pairs, basic)?;
    // Some servers (the MCP Python SDK's, 2.3) want a `client_secret` even
    // from a public client, which has none: it is sent again with an empty
    // one, a form RFC 6749 (section 2.3.1) allows for an empty secret.
    if status == 400 && registration.client_secret.is_none() {
        pairs.push(("client_secret", ""));
        (status, value) = fetch.post_form(endpoint, &pairs, None)?;
    }
    if status == 200 {
        Ok(true)
    } else {
        Err(format!(
            "the authorization server {} did not revoke the token (HTTP {status}): {}",
            meta.issuer,
            describe_error(&value)
        ))
    }
}

/// The union of two scope lists, in order, without repeats.
pub fn union(a: Option<&str>, b: &[String]) -> Option<String> {
    let mut out: Vec<String> = Vec::new();
    for s in a
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_string)
        .chain(b.iter().cloned())
    {
        if !out.contains(&s) {
            out.push(s);
        }
    }
    (!out.is_empty()).then(|| out.join(" "))
}

/// Gives the `Authorization` header an MCP server over HTTP needs, and a
/// new one after the server refused it, where that needs no person.
pub trait Authorizer: Send + Sync + std::fmt::Debug {
    /// The header to send now, if any.
    fn header(&self) -> Result<Option<String>, String>;
    /// The server answered `status` (401, or 403 for too few scopes) with
    /// `challenges`: a header to retry with, if one can be had without a
    /// person (a refresh, a new client credentials token), or why not.
    fn renew(&self, status: u16, challenges: &[Challenge]) -> Result<String, String>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn challenges_are_read_with_their_parameters() {
        let found = challenges([
            r#"Bearer resource_metadata="https://mcp.example.com/.well-known/oauth-protected-resource", scope="files:read files:write""#,
            r#"Basic realm="old", Bearer error="insufficient_scope", scope=admin, error_description="needs \"admin\"""#,
        ]);
        assert_eq!(found.len(), 3);
        assert_eq!(found[0].scheme, "bearer");
        assert_eq!(
            found[0].param("resource_metadata"),
            Some("https://mcp.example.com/.well-known/oauth-protected-resource")
        );
        assert_eq!(found[0].scopes(), ["files:read", "files:write"]);
        assert_eq!(found[1].scheme, "basic");
        assert_eq!(found[2].param("error"), Some("insufficient_scope"));
        assert_eq!(found[2].scopes(), ["admin"]);
        assert_eq!(found[2].param("error_description"), Some("needs \"admin\""));
        assert!(challenges(["Bearer"])[0].params.is_empty());
    }

    #[test]
    fn a_server_at_private_addresses_is_told_from_a_public_one() {
        assert!(is_private("http://127.0.0.1:8812/mcp"));
        assert!(is_private("https://10.1.2.3/mcp"));
        assert!(is_private("https://[::1]:8443/mcp"));
        assert!(is_private("https://192.168.0.7"));
        assert!(!is_private("https://8.8.8.8/mcp"));
        assert!(!is_private("https://[2606:4700:4700::1111]/mcp"));
        // A name that does not resolve here is not taken as private.
        assert!(!is_private("https://no-such-host.invalid/mcp"));
        assert!(!is_private("not a url"));
    }

    #[test]
    fn urls_are_made_canonical_and_resources_cover_their_paths() {
        assert_eq!(
            canonical("HTTPS://MCP.Example.com/mcp/#x").unwrap(),
            "https://mcp.example.com/mcp"
        );
        assert_eq!(
            canonical("https://mcp.example.com/").unwrap(),
            "https://mcp.example.com"
        );
        assert_eq!(
            canonical("https://mcp.example.com:8443?a=1").unwrap(),
            "https://mcp.example.com:8443"
        );
        assert!(canonical("mcp.example.com").is_err());
        assert!(covers(
            "https://mcp.example.com",
            "https://mcp.example.com/mcp"
        ));
        assert!(covers(
            "https://mcp.example.com/mcp",
            "https://mcp.example.com/mcp/"
        ));
        assert!(!covers(
            "https://mcp.example.com/mcp",
            "https://mcp.example.com/mcpx"
        ));
        assert!(!covers(
            "https://other.example.com",
            "https://mcp.example.com/mcp"
        ));
        assert!(!covers(
            "http://mcp.example.com",
            "https://mcp.example.com/mcp"
        ));
        assert!(is_loopback("http://127.0.0.1:8080/mcp"));
        assert!(is_loopback("http://localhost/mcp"));
        assert!(is_loopback("http://[::1]:9/"));
        assert!(!is_loopback("http://10.0.0.1/"));
    }

    #[test]
    fn encoding_round_trips_and_queries_are_read() {
        let text = "a b&c=d/é~";
        assert_eq!(encode(text), "a%20b%26c%3Dd%2F%C3%A9~");
        assert_eq!(decode(&encode(text)), text);
        assert_eq!(decode("a+b%2"), "a b%2");
        let q = query("code=abc%2Fd&state=xyz&iss=https%3A%2F%2Fauth.example.com&empty");
        assert_eq!(q["code"], "abc/d");
        assert_eq!(q["iss"], "https://auth.example.com");
        assert_eq!(q["empty"], "");
    }

    #[test]
    fn pkce_follows_rfc_7636() {
        // The example of RFC 7636, appendix B.
        assert_eq!(
            challenge_of("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        let meta = ServerMetadata {
            issuer: "https://auth.example.com".into(),
            authorization_endpoint: Some("https://auth.example.com/authorize".into()),
            token_endpoint: "https://auth.example.com/token".into(),
            registration_endpoint: None,
            scopes_supported: vec![],
            code_challenge_methods_supported: vec!["S256".into()],
            grant_types_supported: vec![],
            token_endpoint_auth_methods_supported: vec![],
            client_id_metadata_document_supported: false,
            authorization_response_iss_parameter_supported: true,
            revocation_endpoint: None,
            revocation_endpoint_auth_methods_supported: vec![],
        };
        let registration = Registration {
            issuer: meta.issuer.clone(),
            client_id: "client 1".into(),
            client_secret: None,
            redirect_uri: None,
            how: "pre-registered".into(),
        };
        let (url, pending) = begin(
            &meta,
            &registration,
            "http://127.0.0.1:4711/callback",
            "https://mcp.example.com/mcp",
            Some("files:read"),
        )
        .unwrap();
        let q = query(url.split_once('?').unwrap().1);
        assert_eq!(q["code_challenge"], challenge_of(&pending.verifier));
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["resource"], "https://mcp.example.com/mcp");
        assert_eq!(q["client_id"], "client 1");
        assert_eq!(q["scope"], "files:read");
        assert!(pending.verifier.len() >= 43);
        // The answer: issuer first, then state.
        let mut answer = BTreeMap::new();
        answer.insert("code".to_string(), "c0de".to_string());
        answer.insert("state".to_string(), pending.state.clone());
        let err = finish(&pending, &answer).unwrap_err();
        assert!(err.contains("does not say it comes from"), "{err}");
        answer.insert("iss".into(), "https://evil.example".into());
        answer.insert("error".into(), "access_denied".into());
        let err = finish(&pending, &answer).unwrap_err();
        assert!(
            err.contains("not used") && !err.contains("access_denied"),
            "{err}"
        );
        answer.insert("iss".into(), meta.issuer.clone());
        answer.remove("error");
        assert_eq!(finish(&pending, &answer).unwrap(), "c0de");
        answer.insert("state".into(), "other".into());
        assert!(finish(&pending, &answer).is_err());
    }

    #[test]
    fn scopes_are_joined_without_repeats() {
        assert_eq!(
            union(Some("a b"), &["b".into(), "c".into()]).as_deref(),
            Some("a b c")
        );
        assert_eq!(union(None, &[]), None);
    }
}
