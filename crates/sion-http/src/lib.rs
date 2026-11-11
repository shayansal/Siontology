//! Outbound HTTP for Siontology's tools: storage providers fetching a deal's
//! data, and agents calling model providers or fetching pages.
//!
//! Every request made with [`Client`] is bounded and guarded:
//!
//! - **TLS** is rustls with the aws-lc-rs provider, offering the hybrid
//!   post-quantum key exchange X25519MLKEM768 first, then X25519 and P-256
//!   for servers that have no post-quantum key exchange yet. Roots are
//!   Mozilla's (`webpki-roots`) plus any in the PEM file `SSL_CERT_FILE`
//!   names (a corporate or sandbox proxy's).
//! - **Deadlines**: a connect timeout and an overall timeout per request.
//! - **Bounded reads**: a body longer than the caller's limit is an error,
//!   checked against `Content-Length` before reading and against the bytes
//!   read.
//! - **No redirects** unless [`Options::redirects`] allows them.
//! - **No private or local addresses** unless [`Options::allow_private`]:
//!   names are resolved here and loopback, private, link-local, shared,
//!   documentation, multicast and unspecified addresses are dropped, so a
//!   URL written by a stranger cannot reach the machine itself or its local
//!   network. Behind a proxy (`HTTPS_PROXY`, honouring `NO_PROXY`), the
//!   proxy resolves names: then only literal addresses and local names
//!   (`localhost`, `*.local`, `*.internal`) can be refused.

use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

use rustls::crypto::{aws_lc_rs, CryptoProvider};
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, ServerName};

mod digest;
pub mod ws;

pub use digest::{base64, hmac_sha1, hmac_sha256, same_bytes, sha1};

pub mod cipher;

/// What went wrong.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum HttpError {
    /// The URL or its address is not allowed.
    #[error("refused: {0}")]
    Refused(String),
    /// The connection failed or timed out.
    #[error("{0}")]
    Transport(String),
    /// The server answered with an error status.
    #[error("HTTP {status}: {body}")]
    Status {
        /// The status code.
        status: u16,
        /// The start of the body.
        body: String,
        /// How long the server asked to be left before it is asked again,
        /// in seconds (`Retry-After`, or `retry-after-ms` rounded up), if
        /// it said.
        retry_after: Option<u64>,
    },
    /// The body is longer than the limit.
    #[error("the response is longer than {0} bytes")]
    TooLarge(u64),
    /// Configuration (roots, TLS).
    #[error("configuration: {0}")]
    Config(String),
    /// The other side closed a WebSocket connection ([`ws`]).
    #[error("the connection was closed ({code}): {reason}")]
    Closed {
        /// The close code (1005 if none was given).
        code: u16,
        /// The reason given.
        reason: String,
    },
}

/// How a [`Client`] behaves.
#[derive(Clone, Debug)]
pub struct Options {
    /// Connect timeout.
    pub connect_timeout: Duration,
    /// Overall timeout of one request, body included.
    pub timeout: Duration,
    /// If set, a request has no overall deadline: each wait for bytes,
    /// reading or writing, is at most this long instead. For answers that
    /// stream for as long as they take (a model's), where what matters is
    /// that the other side still sends, not how long it has been.
    pub idle_timeout: Option<Duration>,
    /// Redirects followed (0: a redirect is an error).
    pub redirects: u32,
    /// Allow loopback, private and other local addresses.
    pub allow_private: bool,
    /// Use `HTTPS_PROXY` (and `NO_PROXY`) from the environment.
    pub proxy_from_env: bool,
    /// The `User-Agent` header.
    pub user_agent: String,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            connect_timeout: Duration::from_secs(5),
            timeout: Duration::from_secs(30),
            idle_timeout: None,
            redirects: 0,
            allow_private: false,
            proxy_from_env: true,
            user_agent: format!("sion/{}", env!("CARGO_PKG_VERSION")),
        }
    }
}

/// A response read in full, within its limit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fetched {
    /// The status code (2xx).
    pub status: u16,
    /// The `Content-Type`, if any.
    pub content_type: Option<String>,
    /// The body.
    pub body: Vec<u8>,
}

/// A guarded HTTP client (see the crate documentation).
#[derive(Clone, Debug)]
pub struct Client {
    options: Options,
    direct: ureq::Agent,
    proxied: Option<(ureq::Agent, NoProxy)>,
    /// The proxy's `host:port`, and its `Proxy-Authorization` if its URL
    /// names a user, for connections made here ([`Client::connect`]).
    tunnel: Option<(String, Option<String>)>,
    config: Arc<ClientConfig>,
}

impl Client {
    /// A client with `options`.
    pub fn new(options: Options) -> Result<Client, HttpError> {
        let config = tls_config()?;
        let tls = Arc::new(PostQuantumTls {
            config: config.clone(),
        });
        let build = |proxy: Option<(&str, ureq::Proxy)>| {
            let mut allowed = Vec::new();
            let builder = ureq::AgentBuilder::new().timeout_connect(options.connect_timeout);
            let mut builder = match options.idle_timeout {
                Some(idle) => builder.timeout_read(idle).timeout_write(idle),
                None => builder.timeout(options.timeout),
            }
            .redirects(options.redirects)
            .user_agent(&options.user_agent)
            .tls_connector(tls.clone());
            if let Some((netloc, proxy)) = proxy {
                allowed.push(netloc.to_string());
                builder = builder.proxy(proxy);
            }
            builder
                .resolver(GuardedResolver {
                    allow_private: options.allow_private,
                    allowed,
                })
                .build()
        };
        let direct = build(None);
        let found = if options.proxy_from_env {
            proxy_from_env()
        } else {
            None
        };
        let proxied = found
            .as_ref()
            .map(|(url, netloc)| -> Result<_, HttpError> {
                let proxy = ureq::Proxy::new(url)
                    .map_err(|e| HttpError::Config(format!("proxy {url}: {e}")))?;
                Ok((build(Some((netloc, proxy))), NoProxy::from_env()))
            })
            .transpose()?;
        let tunnel = found.map(|(url, netloc)| (netloc, proxy_authorization(&url)));
        Ok(Client {
            options,
            direct,
            proxied,
            tunnel,
            config,
        })
    }

    /// A connection of its own to `url`'s host, for a protocol that takes
    /// over from HTTP (WebSocket: `ws`, `wss`, `http` or `https` URLs) or
    /// has its own (IRC: `irc`, `ircs`; mail: `imap`, `imaps`, `smtp`,
    /// `smtps`): TLS for `wss`, `https`, `ircs`, `imaps` and `smtps`, with
    /// the client's roots, address rules, connect timeout and proxy (a
    /// `CONNECT` tunnel, unless `NO_PROXY` names the host). The others
    /// start in the clear, for a protocol that turns to TLS itself
    /// ([`Client::start_tls`]). Reads and writes time out after the
    /// client's timeout, until the caller sets another.
    pub fn connect(&self, url: &str) -> Result<Stream, HttpError> {
        let target = Target::parse(url)?;
        let through = match (&self.tunnel, &self.proxied) {
            (Some(tunnel), Some((_, no_proxy))) if !no_proxy.matches(&target.host) => Some(tunnel),
            _ => None,
        };
        let tcp = match through {
            Some((proxy, authorization)) => {
                // The proxy resolves the name; refuse what can be seen here.
                if !self.options.allow_private && !public_host(&target.host) {
                    return Err(HttpError::Refused(format!(
                        "{} is a local or private address",
                        target.host
                    )));
                }
                let tcp = connect_to(proxy, true, self.options.connect_timeout)?;
                tunnel(tcp, &target, authorization.as_deref(), self.options.timeout)?
            }
            None => connect_to(
                &target.netloc(),
                self.options.allow_private,
                self.options.connect_timeout,
            )?,
        };
        let failed = |e: std::io::Error| HttpError::Transport(e.to_string());
        tcp.set_read_timeout(Some(self.options.timeout))
            .map_err(failed)?;
        tcp.set_write_timeout(Some(self.options.timeout))
            .map_err(failed)?;
        let _ = tcp.set_nodelay(true);
        let plain = Stream {
            inner: Inner::Plain(tcp),
            host: target.host,
        };
        if !target.tls {
            return Ok(plain);
        }
        self.start_tls(plain)
    }

    /// `stream`, a connection [`Client::connect`] made in the clear, as TLS
    /// from here on: for a protocol's `STARTTLS`, once the server has agreed
    /// to it. The server is checked as the host the connection was made
    /// to, with the client's roots; the handshake is made with the first
    /// read or write.
    pub fn start_tls(&self, stream: Stream) -> Result<Stream, HttpError> {
        let Stream { inner, host } = stream;
        let tcp = match inner {
            Inner::Plain(tcp) => tcp,
            Inner::Tls(_) => {
                return Err(HttpError::Refused(format!(
                    "the connection to {host} is TLS already"
                )))
            }
        };
        let name = ServerName::try_from(host.clone())
            .map_err(|e| HttpError::Refused(format!("{host}: {e}")))?;
        let connection = ClientConnection::new(self.config.clone(), name)
            .map_err(|e| HttpError::Config(e.to_string()))?;
        Ok(Stream {
            inner: Inner::Tls(Box::new(StreamOwned::new(connection, tcp))),
            host,
        })
    }

    /// The options.
    pub fn options(&self) -> &Options {
        &self.options
    }

    /// `GET url`, reading at most `max_bytes` of body.
    pub fn get(&self, url: &str, max_bytes: u64) -> Result<Fetched, HttpError> {
        self.request("GET", url, &[], None, max_bytes)
    }

    /// `POST url` with a JSON body and extra headers, reading at most
    /// `max_bytes` of the answer. An error status is returned as
    /// [`HttpError::Status`] with the start of its body.
    pub fn post_json(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        body: &[u8],
        max_bytes: u64,
    ) -> Result<Fetched, HttpError> {
        let mut all = vec![("Content-Type", "application/json")];
        all.extend_from_slice(headers);
        self.request("POST", url, &all, Some(body), max_bytes)
    }

    /// The agent for a request to `url`: through the proxy unless
    /// `NO_PROXY` names its host.
    fn agent_for(&self, url: &str) -> Result<&ureq::Agent, HttpError> {
        let host = check_url(url)?;
        Ok(match &self.proxied {
            Some((agent, no_proxy)) if !no_proxy.matches(&host) => {
                // The proxy resolves the name; refuse what can be seen here.
                if !self.options.allow_private && !public_host(&host) {
                    return Err(HttpError::Refused(format!(
                        "{host} is a local or private address"
                    )));
                }
                agent
            }
            _ => &self.direct,
        })
    }

    /// One request, answered whatever its status: the status, the headers
    /// and the body as it arrives, for protocols that read the headers of
    /// an error (`WWW-Authenticate`) or a body that streams (Server-Sent
    /// Events). The caller bounds what it reads; the client's timeout
    /// bounds how long the whole response may take.
    pub fn send(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, &str)],
        body: Option<&[u8]>,
    ) -> Result<Response, HttpError> {
        let agent = self.agent_for(url)?;
        let mut request = agent.request(method, url);
        for (name, value) in headers {
            request = request.set(name, value);
        }
        let result = match body {
            Some(bytes) => request.send_bytes(bytes),
            None => request.call(),
        };
        let response = match result {
            Ok(response) | Err(ureq::Error::Status(_, response)) => response,
            Err(ureq::Error::Transport(transport)) => {
                return Err(HttpError::Transport(transport.to_string()));
            }
        };
        let status = response.status();
        // ureq names a header once per spelling and gives every value of
        // it whatever the spelling: each name once, lower-cased.
        let mut names: Vec<String> = response
            .headers_names()
            .into_iter()
            .map(|name| name.to_ascii_lowercase())
            .collect();
        names.sort();
        names.dedup();
        let headers = names
            .into_iter()
            .flat_map(|name| {
                response
                    .all(&name)
                    .into_iter()
                    .map(|value| (name.clone(), value.to_string()))
                    .collect::<Vec<_>>()
            })
            .collect();
        Ok(Response {
            status,
            headers,
            body: response.into_reader(),
        })
    }

    /// One request: `method url` with `headers` and an optional body.
    pub fn request(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, &str)],
        body: Option<&[u8]>,
        max_bytes: u64,
    ) -> Result<Fetched, HttpError> {
        let agent = self.agent_for(url)?;
        let mut request = agent.request(method, url);
        for (name, value) in headers {
            request = request.set(name, value);
        }
        let result = match body {
            Some(bytes) => request.send_bytes(bytes),
            None => request.call(),
        };
        let response = match result {
            Ok(response) => response,
            Err(ureq::Error::Status(status, response)) => {
                let retry_after = retry_after(
                    response.header("retry-after"),
                    response.header("retry-after-ms"),
                );
                let mut start = Vec::new();
                let _ = response.into_reader().take(2_048).read_to_end(&mut start);
                return Err(HttpError::Status {
                    status,
                    body: String::from_utf8_lossy(&start).into_owned(),
                    retry_after,
                });
            }
            Err(ureq::Error::Transport(transport)) => {
                return Err(HttpError::Transport(transport.to_string()));
            }
        };
        let status = response.status();
        if !(200..300).contains(&status) {
            return Err(HttpError::Status {
                status,
                body: format!(
                    "not followed (to {})",
                    response.header("location").unwrap_or("nowhere")
                ),
                retry_after: None,
            });
        }
        if let Some(length) = response
            .header("content-length")
            .and_then(|v| v.trim().parse::<u64>().ok())
        {
            if length > max_bytes {
                return Err(HttpError::TooLarge(max_bytes));
            }
        }
        let content_type = response.header("content-type").map(str::to_string);
        let mut body = Vec::new();
        response
            .into_reader()
            .take(max_bytes.saturating_add(1))
            .read_to_end(&mut body)
            .map_err(|e| HttpError::Transport(e.to_string()))?;
        if body.len() as u64 > max_bytes {
            return Err(HttpError::TooLarge(max_bytes));
        }
        Ok(Fetched {
            status,
            content_type,
            body,
        })
    }
}

/// A response whose body is read as it arrives ([`Client::send`]).
/// Whether `error` may pass if the request is made again: the connection
/// failed, or the server was busy or failing (408, 425, 429, 500, 502, 503,
/// 504, 529).
pub fn transient(error: &HttpError) -> bool {
    match error {
        HttpError::Transport(_) => true,
        HttpError::Status { status, .. } => {
            matches!(status, 408 | 425 | 429 | 500 | 502 | 503 | 504 | 529)
        }
        _ => false,
    }
}

/// `request` made, and made again at most `retries` times while it fails
/// in a way that may pass ([`transient`]): after the pause the server asks
/// for (`Retry-After`) or else 1, 2, 4… seconds, each pause at most
/// `max_pause` seconds; `pause` waits (`std::thread::sleep` in use, a
/// counter in tests). Only for requests that can be repeated safely.
pub fn retrying<T>(
    retries: u32,
    max_pause: u64,
    pause: &dyn Fn(u64),
    request: &dyn Fn() -> Result<T, HttpError>,
) -> Result<T, HttpError> {
    let mut tried = 0;
    loop {
        match request() {
            Err(error) if tried < retries && transient(&error) => {
                let asked = match &error {
                    HttpError::Status { retry_after, .. } => *retry_after,
                    _ => None,
                };
                let wait = asked
                    .unwrap_or(1 << tried.min(16))
                    .clamp(1, max_pause.max(1));
                pause(wait);
                tried += 1;
            }
            answered => return answered,
        }
    }
}

/// Waits `seconds`: [`retrying`]'s pause in use.
pub fn sleep_seconds(seconds: u64) {
    std::thread::sleep(std::time::Duration::from_secs(seconds));
}

/// How long a server asks to be left, in seconds, from its `Retry-After`
/// (seconds; a date is not read) and `retry-after-ms` headers: the
/// milliseconds rounded up, if it gave both or only them.
pub fn retry_after(seconds: Option<&str>, millis: Option<&str>) -> Option<u64> {
    let millis = millis
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|ms| ms.is_finite() && *ms >= 0.0)
        .map(|ms| (ms / 1_000.0).ceil() as u64);
    let seconds = seconds
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|s| s.is_finite() && *s >= 0.0)
        .map(|s| s.ceil() as u64);
    millis.or(seconds)
}

pub struct Response {
    /// The status code.
    pub status: u16,
    headers: Vec<(String, String)>,
    body: Box<dyn Read + Send + Sync + 'static>,
}

impl std::fmt::Debug for Response {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Response")
            .field("status", &self.status)
            .field("headers", &self.headers)
            .finish_non_exhaustive()
    }
}

impl Response {
    /// The first value of the header `name` (case is ignored).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// Every value of the header `name` (case is ignored).
    pub fn all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.headers
            .iter()
            .filter(move |(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// The media type of the body, lower-cased, without its parameters.
    pub fn media_type(&self) -> Option<String> {
        self.header("content-type").map(|v| {
            v.split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase()
        })
    }

    /// The body, to read as it arrives.
    pub fn body(&mut self) -> &mut Box<dyn Read + Send + Sync + 'static> {
        &mut self.body
    }

    /// The body, read in full: at most `max_bytes` of it.
    pub fn read_all(mut self, max_bytes: u64) -> Result<Vec<u8>, HttpError> {
        let mut body = Vec::new();
        (&mut self.body)
            .take(max_bytes.saturating_add(1))
            .read_to_end(&mut body)
            .map_err(|e| HttpError::Transport(e.to_string()))?;
        if body.len() as u64 > max_bytes {
            return Err(HttpError::TooLarge(max_bytes));
        }
        Ok(body)
    }
}

/// The SHA-256 digest of `data`, by the TLS stack's own implementation
/// (aws-lc-rs), for protocols that name it (PKCE).
pub fn sha256(data: &[u8]) -> [u8; 32] {
    let suite = aws_lc_rs::cipher_suite::TLS13_AES_128_GCM_SHA256
        .tls13()
        .expect("a TLS 1.3 suite");
    let digest = suite.common.hash_provider.hash(data);
    let mut out = [0u8; 32];
    out.copy_from_slice(digest.as_ref());
    out
}

/// Fills `buf` with random bytes from the operating system, through the
/// TLS stack's generator (aws-lc-rs).
pub fn random(buf: &mut [u8]) -> Result<(), HttpError> {
    aws_lc_rs::default_provider()
        .secure_random
        .fill(buf)
        .map_err(|_| HttpError::Config("the system gave no random bytes".into()))
}

/// A DER length.
fn der_length(len: usize) -> Vec<u8> {
    if len < 0x80 {
        return vec![len as u8];
    }
    let bytes: Vec<u8> = len
        .to_be_bytes()
        .into_iter()
        .skip_while(|b| *b == 0)
        .collect();
    let mut out = vec![0x80 | bytes.len() as u8];
    out.extend(bytes);
    out
}

/// A DER INTEGER of the unsigned big-endian `bytes`.
fn der_unsigned(bytes: &[u8]) -> Vec<u8> {
    let mut value: Vec<u8> = bytes.iter().copied().skip_while(|b| *b == 0).collect();
    if value.first().map_or(true, |b| b & 0x80 != 0) {
        value.insert(0, 0);
    }
    let mut out = vec![0x02];
    out.extend(der_length(value.len()));
    out.extend(value);
    out
}

/// Whether `signature` is RSASSA-PKCS1-v1_5 with SHA-256 (JWT's `RS256`)
/// over `message` by the RSA key of modulus `n` and exponent `e`
/// (big-endian), of 2048 to 8192 bits, checked by the TLS stack
/// (aws-lc-rs) as it checks certificates.
pub fn rsa_sha256_verify(n: &[u8], e: &[u8], message: &[u8], signature: &[u8]) -> bool {
    // The key as an RSAPublicKey (RFC 8017): SEQUENCE { n, e }.
    let mut body = der_unsigned(n);
    body.extend(der_unsigned(e));
    let mut key = vec![0x30];
    key.extend(der_length(body.len()));
    key.extend(body);
    aws_lc_rs::default_provider()
        .signature_verification_algorithms
        .mapping
        .iter()
        .filter(|(scheme, _)| *scheme == rustls::SignatureScheme::RSA_PKCS1_SHA256)
        .flat_map(|(_, algorithms)| algorithms.iter())
        .any(|algorithm| algorithm.verify_signature(&key, message, signature).is_ok())
}

/// RSASSA-PKCS1-v1_5 with SHA-256 (`RS256`) over `message`, by the RSA
/// private key in `pem` (PKCS#8 or PKCS#1), with the TLS stack.
pub fn rsa_sha256_sign(pem: &str, message: &[u8]) -> Result<Vec<u8>, HttpError> {
    let refused = |e: &dyn std::fmt::Display| HttpError::Config(format!("the private key: {e}"));
    let der =
        rustls_pki_types::PrivateKeyDer::from_pem_slice(pem.as_bytes()).map_err(|e| refused(&e))?;
    let key = aws_lc_rs::default_provider()
        .key_provider
        .load_private_key(der)
        .map_err(|e| refused(&e))?;
    let signer = key
        .choose_scheme(&[rustls::SignatureScheme::RSA_PKCS1_SHA256])
        .ok_or_else(|| refused(&"not an RSA key"))?;
    signer.sign(message).map_err(|e| refused(&e))
}

/// Where a connection goes ([`Client::connect`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Target {
    /// The host, lower-cased, without brackets.
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) tls: bool,
    /// The path and query (`/` if the URL has none).
    pub(crate) path: String,
}

impl Target {
    /// The target of a `ws`, `wss`, `http`, `https`, `irc`, `ircs`, `imap`,
    /// `imaps`, `smtp` or `smtps` URL (`smtp` is mail submission, port 587
    /// by default).
    pub(crate) fn parse(url: &str) -> Result<Target, HttpError> {
        let (scheme, rest) = url
            .split_once("://")
            .ok_or_else(|| HttpError::Refused(format!("{url}: not a URL")))?;
        let (tls, default_port) = match scheme.to_ascii_lowercase().as_str() {
            "wss" | "https" => (true, 443),
            "ws" | "http" => (false, 80),
            "ircs" => (true, 6697),
            "irc" => (false, 6667),
            "imaps" => (true, 993),
            "imap" => (false, 143),
            "smtps" => (true, 465),
            "smtp" => (false, 587),
            _ => {
                return Err(HttpError::Refused(format!(
                    "{url}: only ws, wss, http, https, irc, ircs, imap, imaps, smtp and smtps URLs"
                )))
            }
        };
        let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let (authority, after) = rest.split_at(end);
        if authority.contains('@') {
            return Err(HttpError::Refused(format!(
                "{url}: credentials do not go in the URL"
            )));
        }
        let (host, port) = if let Some(bracketed) = authority.strip_prefix('[') {
            let (host, after) = bracketed
                .split_once(']')
                .ok_or_else(|| HttpError::Refused(format!("{url}: an unclosed bracket")))?;
            (host, after.strip_prefix(':'))
        } else {
            match authority.rsplit_once(':') {
                Some((host, port)) => (host, Some(port)),
                None => (authority, None),
            }
        };
        if host.is_empty() {
            return Err(HttpError::Refused(format!("{url}: no host")));
        }
        let port = match port {
            Some(port) => port
                .parse()
                .map_err(|_| HttpError::Refused(format!("{url}: not a port")))?,
            None => default_port,
        };
        let path = after.split('#').next().unwrap_or("");
        let path = if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{path}")
        };
        Ok(Target {
            host: host.to_ascii_lowercase(),
            port,
            tls,
            path,
        })
    }

    /// `host:port`, an IPv6 address in brackets.
    pub(crate) fn netloc(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }

    /// The `Host` header: the port only where it is not the scheme's own.
    pub(crate) fn host_header(&self) -> String {
        let default = if self.tls { 443 } else { 80 };
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        if self.port == default {
            host
        } else {
            format!("{host}:{}", self.port)
        }
    }
}

/// A connection made by [`Client::connect`]: TCP, or TLS over TCP.
pub struct Stream {
    inner: Inner,
    /// The host it was made to, lower-cased, without brackets.
    host: String,
}

enum Inner {
    Plain(TcpStream),
    Tls(Box<StreamOwned<ClientConnection, TcpStream>>),
}

impl std::fmt::Debug for Stream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.inner {
            Inner::Plain(_) => write!(f, "Stream(tcp, {})", self.host),
            Inner::Tls(_) => write!(f, "Stream(tls, {})", self.host),
        }
    }
}

impl Stream {
    /// How long a read waits for bytes (`None`: as long as it takes).
    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()> {
        match &self.inner {
            Inner::Plain(tcp) => tcp.set_read_timeout(timeout),
            Inner::Tls(tls) => tls.sock.set_read_timeout(timeout),
        }
    }

    /// Whether it is TLS.
    pub fn is_tls(&self) -> bool {
        matches!(self.inner, Inner::Tls(_))
    }

    /// The host it was made to, as its URL names it (lower-cased, an IPv6
    /// address without brackets).
    pub fn host(&self) -> &str {
        &self.host
    }
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match &mut self.inner {
            Inner::Plain(tcp) => tcp.read(buf),
            Inner::Tls(tls) => tls.read(buf),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match &mut self.inner {
            Inner::Plain(tcp) => tcp.write(buf),
            Inner::Tls(tls) => tls.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match &mut self.inner {
            Inner::Plain(tcp) => tcp.flush(),
            Inner::Tls(tls) => tls.flush(),
        }
    }
}

/// A TCP connection to `netloc`, resolved here and held to the address
/// rules unless `allow_private`; each address in turn.
fn connect_to(
    netloc: &str,
    allow_private: bool,
    timeout: Duration,
) -> Result<TcpStream, HttpError> {
    let addresses = ureq::Resolver::resolve(
        &GuardedResolver {
            allow_private,
            allowed: Vec::new(),
        },
        netloc,
    )
    .map_err(|e| HttpError::Transport(format!("{netloc}: {e}")))?;
    let mut last = format!("{netloc}: no address");
    for address in addresses {
        match TcpStream::connect_timeout(&address, timeout) {
            Ok(tcp) => return Ok(tcp),
            Err(e) => last = format!("{netloc}: {e}"),
        }
    }
    Err(HttpError::Transport(last))
}

/// A tunnel through an HTTP proxy to `target` (`CONNECT`).
fn tunnel(
    mut tcp: TcpStream,
    target: &Target,
    authorization: Option<&str>,
    timeout: Duration,
) -> Result<TcpStream, HttpError> {
    let failed = |e: std::io::Error| HttpError::Transport(format!("the proxy: {e}"));
    tcp.set_read_timeout(Some(timeout)).map_err(failed)?;
    let netloc = target.netloc();
    let mut request = format!("CONNECT {netloc} HTTP/1.1\r\nHost: {netloc}\r\n");
    if let Some(authorization) = authorization {
        request.push_str(&format!("Proxy-Authorization: {authorization}\r\n"));
    }
    request.push_str("\r\n");
    tcp.write_all(request.as_bytes()).map_err(failed)?;
    // The answer's head, a byte at a time, so that nothing of the tunnel's
    // own traffic is read with it.
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() > 8 << 10 {
            return Err(HttpError::Transport(
                "the proxy's answer is too long".into(),
            ));
        }
        let mut byte = [0u8; 1];
        if tcp.read(&mut byte).map_err(failed)? == 0 {
            return Err(HttpError::Transport(
                "the proxy closed the connection".into(),
            ));
        }
        head.push(byte[0]);
    }
    let head = String::from_utf8_lossy(&head);
    let status = head.lines().next().unwrap_or_default();
    if status.split_whitespace().nth(1) != Some("200") {
        return Err(HttpError::Transport(format!(
            "the proxy refused a tunnel to {netloc}: {status}"
        )));
    }
    Ok(tcp)
}

/// `Proxy-Authorization` for a proxy URL that names a user
/// (`http://user:password@proxy:3128`), percent-decoded.
fn proxy_authorization(url: &str) -> Option<String> {
    let rest = url.split("://").nth(1).unwrap_or(url);
    let authority = rest.split('/').next().unwrap_or(rest);
    let (user, _) = authority.rsplit_once('@')?;
    let mut decoded = Vec::new();
    let bytes = user.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| char::from(b).to_digit(16);
        match (bytes[i], bytes.get(i + 1), bytes.get(i + 2)) {
            (b'%', Some(&h), Some(&l)) if hex(h).is_some() && hex(l).is_some() => {
                decoded.push((hex(h)? * 16 + hex(l)?) as u8);
                i += 3;
            }
            (b, _, _) => {
                decoded.push(b);
                i += 1;
            }
        }
    }
    Some(format!("Basic {}", base64(&decoded)))
}

/// The host of an `http` or `https` URL, lower-cased, without brackets.
fn check_url(url: &str) -> Result<String, HttpError> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .ok_or_else(|| HttpError::Refused(format!("{url}: only http and https URLs")))?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let authority = authority.rsplit('@').next().unwrap_or("");
    let host = if let Some(bracketed) = authority.strip_prefix('[') {
        bracketed.split(']').next().unwrap_or("")
    } else {
        authority.split(':').next().unwrap_or("")
    };
    if host.is_empty() {
        return Err(HttpError::Refused(format!("{url}: no host")));
    }
    Ok(host.to_ascii_lowercase())
}

/// Whether a host name or literal address may be reached when the name is
/// resolved elsewhere (by a proxy).
fn public_host(host: &str) -> bool {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return is_public(ip);
    }
    let host = host.trim_end_matches('.');
    !(host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.ends_with(".internal")
        || !host.contains('.'))
}

/// Whether `ip` is a public unicast address: not loopback, private,
/// link-local, shared (CGNAT), documentation, benchmarking, multicast,
/// broadcast, reserved or unspecified, nor such an address mapped into
/// IPv6.
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, c, _] = v4.octets();
            !(v4.is_unspecified()
                || v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.is_documentation()
                || a == 0
                || (a == 100 && (64..128).contains(&b))
                || (a == 192 && b == 0 && c == 0)
                || (a == 198 && (b == 18 || b == 19))
                || a >= 240)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public(IpAddr::V4(v4));
            }
            let first = v6.segments()[0];
            !(v6.is_unspecified()
                || v6.is_loopback()
                || v6.is_multicast()
                || (first & 0xfe00) == 0xfc00
                || (first & 0xffc0) == 0xfe80
                || first == 0x2001 && v6.segments()[1] == 0x0db8
                || first == 0x0064 && v6.segments()[1] == 0xff9b)
        }
    }
}

/// Resolves names here and keeps only public addresses (unless allowed),
/// so a connection cannot reach a local address by name, rebinding
/// included: what is resolved is what is connected to.
#[derive(Debug)]
struct GuardedResolver {
    allow_private: bool,
    /// Network locations resolved without the check (the proxy's).
    allowed: Vec<String>,
}

impl ureq::Resolver for GuardedResolver {
    fn resolve(&self, netloc: &str) -> std::io::Result<Vec<SocketAddr>> {
        let addresses: Vec<SocketAddr> = netloc.to_socket_addrs()?.collect();
        if self.allow_private || self.allowed.iter().any(|a| a == netloc) {
            return Ok(addresses);
        }
        let public: Vec<SocketAddr> = addresses
            .into_iter()
            .filter(|address| is_public(address.ip()))
            .collect();
        if public.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!("{netloc} resolves only to local or private addresses"),
            ));
        }
        Ok(public)
    }
}

/// The proxy URL and its `host:port` from `HTTPS_PROXY` (or `ALL_PROXY`).
fn proxy_from_env() -> Option<(String, String)> {
    let url = ["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"]
        .iter()
        .find_map(|name| std::env::var(name).ok().filter(|v| !v.trim().is_empty()))?;
    let rest = url.split("://").nth(1).unwrap_or(&url);
    let authority = rest.split('/').next().unwrap_or(rest);
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    let netloc = if authority.contains(':') {
        authority.to_string()
    } else {
        format!("{authority}:80")
    };
    Some((url, netloc))
}

/// Hosts that bypass the proxy (`NO_PROXY`): exact names, suffixes, `*`.
#[derive(Clone, Debug, Default)]
struct NoProxy(Vec<String>);

impl NoProxy {
    fn from_env() -> NoProxy {
        let list = std::env::var("NO_PROXY")
            .or_else(|_| std::env::var("no_proxy"))
            .unwrap_or_default();
        NoProxy(
            list.split(',')
                .map(|entry| entry.trim().trim_start_matches('.').to_ascii_lowercase())
                .filter(|entry| !entry.is_empty())
                .collect(),
        )
    }

    fn matches(&self, host: &str) -> bool {
        self.0
            .iter()
            .any(|entry| entry == "*" || host == entry || host.ends_with(&format!(".{entry}")))
    }
}

/// The TLS configuration: aws-lc-rs, X25519MLKEM768 first.
fn tls_config() -> Result<Arc<ClientConfig>, HttpError> {
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    if let Some(path) = std::env::var_os("SSL_CERT_FILE") {
        let certificates = CertificateDer::pem_file_iter(&path)
            .map_err(|e| HttpError::Config(format!("SSL_CERT_FILE: {e}")))?;
        for certificate in certificates {
            let certificate =
                certificate.map_err(|e| HttpError::Config(format!("SSL_CERT_FILE: {e}")))?;
            // A certificate rustls cannot use as a root is skipped, as
            // other clients do.
            let _ = roots.add(certificate);
        }
    }
    let base = aws_lc_rs::default_provider();
    let provider = CryptoProvider {
        kx_groups: vec![
            aws_lc_rs::kx_group::X25519MLKEM768,
            aws_lc_rs::kx_group::X25519,
            aws_lc_rs::kx_group::SECP256R1,
        ],
        ..base
    };
    let config = ClientConfig::builder_with_provider(Arc::new(provider))
        .with_safe_default_protocol_versions()
        .map_err(|e| HttpError::Config(e.to_string()))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(config))
}

/// The TLS connector ureq uses.
#[derive(Debug)]
struct PostQuantumTls {
    config: Arc<ClientConfig>,
}

impl ureq::TlsConnector for PostQuantumTls {
    fn connect(
        &self,
        dns_name: &str,
        io: Box<dyn ureq::ReadWrite>,
    ) -> Result<Box<dyn ureq::ReadWrite>, ureq::Error> {
        let name = ServerName::try_from(dns_name.to_string()).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("{dns_name}: {e}"))
        })?;
        let connection = ClientConnection::new(self.config.clone(), name)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        Ok(Box::new(TlsStream(StreamOwned::new(connection, io))))
    }
}

/// A TLS stream over ureq's transport.
struct TlsStream(StreamOwned<ClientConnection, Box<dyn ureq::ReadWrite>>);

impl std::fmt::Debug for TlsStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TlsStream").finish_non_exhaustive()
    }
}

impl Read for TlsStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(buf)
    }
}

impl Write for TlsStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

impl ureq::ReadWrite for TlsStream {
    fn socket(&self) -> Option<&TcpStream> {
        self.0.get_ref().socket()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failure_that_may_pass_is_tried_again_after_the_pause_asked() {
        assert_eq!(retry_after(Some("7"), None), Some(7));
        assert_eq!(retry_after(Some("1.5"), Some("2500")), Some(3));
        assert_eq!(
            retry_after(Some("Wed, 21 Oct 2026 07:28:00 GMT"), None),
            None
        );
        assert_eq!(retry_after(Some("-1"), None), None);
        let paused = std::cell::RefCell::new(Vec::new());
        let tries = std::cell::Cell::new(0);
        let answered: Result<u32, HttpError> =
            retrying(3, 30, &|s| paused.borrow_mut().push(s), &|| {
                tries.set(tries.get() + 1);
                match tries.get() {
                    1 => Err(HttpError::Status {
                        status: 429,
                        body: "slow down".into(),
                        retry_after: Some(120),
                    }),
                    2 => Err(HttpError::Transport("reset".into())),
                    _ => Ok(5),
                }
            });
        assert_eq!(answered, Ok(5));
        // The pause asked for, held to the most; then 2 seconds.
        assert_eq!(*paused.borrow(), vec![30, 2]);
        // A refusal is not tried again.
        tries.set(0);
        let refused: Result<u32, HttpError> = retrying(3, 30, &|_| {}, &|| {
            tries.set(tries.get() + 1);
            Err(HttpError::Status {
                status: 404,
                body: "gone".into(),
                retry_after: None,
            })
        });
        assert!(refused.is_err());
        assert_eq!(tries.get(), 1);
    }

    #[test]
    fn local_and_private_addresses_are_not_public() {
        for text in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "255.255.255.255",
            "224.0.0.1",
            "192.0.2.1",
            "198.18.0.1",
            "::1",
            "::",
            "fe80::1",
            "fd00::1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "2001:db8::1",
        ] {
            assert!(!is_public(text.parse().unwrap()), "{text}");
        }
        for text in ["1.1.1.1", "93.184.216.34", "2606:4700:4700::1111"] {
            assert!(is_public(text.parse().unwrap()), "{text}");
        }
    }

    #[test]
    fn urls_give_their_host_and_other_schemes_are_refused() {
        assert_eq!(
            check_url("https://Example.org:8443/a?b").unwrap(),
            "example.org"
        );
        assert_eq!(check_url("http://user@[::1]:80/").unwrap(), "::1");
        assert!(check_url("file:///etc/passwd").is_err());
        assert!(check_url("ftp://example.org/").is_err());
        assert!(check_url("http:///nohost").is_err());
        assert!(!public_host("localhost"));
        assert!(!public_host("printer.local"));
        assert!(!public_host("intranet"));
        assert!(!public_host("10.0.0.1"));
        assert!(public_host("example.org"));
    }

    #[test]
    fn names_that_resolve_only_locally_are_refused_unless_allowed() {
        let guarded = GuardedResolver {
            allow_private: false,
            allowed: Vec::new(),
        };
        let error = ureq::Resolver::resolve(&guarded, "127.0.0.1:80").unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        let open = GuardedResolver {
            allow_private: true,
            allowed: Vec::new(),
        };
        assert!(!ureq::Resolver::resolve(&open, "127.0.0.1:80")
            .unwrap()
            .is_empty());
        let proxy = GuardedResolver {
            allow_private: false,
            allowed: vec!["127.0.0.1:3128".into()],
        };
        assert!(ureq::Resolver::resolve(&proxy, "127.0.0.1:3128").is_ok());
    }

    #[test]
    fn no_proxy_matches_names_and_suffixes() {
        let list = NoProxy(vec!["localhost".into(), "example.org".into()]);
        assert!(list.matches("localhost"));
        assert!(list.matches("api.example.org"));
        assert!(!list.matches("example.com"));
        assert!(NoProxy(vec!["*".into()]).matches("anything"));
    }

    /// A local server answering with whatever it is given: the client's
    /// bounds, refusals and status handling, without the network.
    fn serve(answers: Vec<String>) -> (u16, std::thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            for answer in answers {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buffer = [0u8; 4096];
                let _ = stream.read(&mut buffer);
                let _ = stream.write_all(answer.as_bytes());
            }
        });
        (port, handle)
    }

    fn local_client() -> Client {
        Client::new(Options {
            allow_private: true,
            proxy_from_env: false,
            timeout: Duration::from_secs(5),
            ..Options::default()
        })
        .unwrap()
    }

    #[test]
    fn bodies_are_bounded_redirects_refused_and_errors_reported() {
        let ok = "HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-length: 5\r\n\r\nhello";
        let long = "HTTP/1.1 200 OK\r\ncontent-length: 100\r\n\r\n".to_string() + &"x".repeat(100);
        let unsized_long =
            "HTTP/1.1 200 OK\r\nconnection: close\r\n\r\n".to_string() + &"y".repeat(50);
        let redirect =
            "HTTP/1.1 302 Found\r\nlocation: http://10.0.0.1/\r\ncontent-length: 0\r\n\r\n";
        let missing = "HTTP/1.1 404 Not Found\r\ncontent-length: 4\r\n\r\nnope";
        let (port, handle) = serve(vec![
            ok.into(),
            long,
            unsized_long,
            redirect.into(),
            missing.into(),
        ]);
        let client = local_client();
        let url = format!("http://127.0.0.1:{port}/");
        let fetched = client.get(&url, 10).unwrap();
        assert_eq!(fetched.body, b"hello");
        assert_eq!(fetched.content_type.as_deref(), Some("text/plain"));
        assert_eq!(client.get(&url, 10), Err(HttpError::TooLarge(10)));
        assert_eq!(client.get(&url, 10), Err(HttpError::TooLarge(10)));
        assert!(matches!(
            client.get(&url, 10),
            Err(HttpError::Status { status: 302, .. })
        ));
        assert_eq!(
            client.get(&url, 10),
            Err(HttpError::Status {
                status: 404,
                body: "nope".into(),
                retry_after: None,
            })
        );
        handle.join().unwrap();
    }

    #[test]
    fn digests_and_random_bytes_come_from_the_tls_stack() {
        let hex: String = sha256(b"abc").iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let (mut a, mut b) = ([0u8; 32], [0u8; 32]);
        random(&mut a).unwrap();
        random(&mut b).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn a_sent_request_gives_any_status_with_its_headers() {
        let unauthorized = "HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Bearer realm=\"x\"\r\nwww-authenticate: Basic\r\ncontent-type: Application/JSON; charset=utf-8\r\ncontent-length: 2\r\n\r\n{}";
        let (port, handle) = serve(vec![unauthorized.into()]);
        let client = local_client();
        let url = format!("http://127.0.0.1:{port}/mcp");
        let response = client
            .send(
                "POST",
                &url,
                &[("Content-Type", "application/json")],
                Some(b"{}"),
            )
            .unwrap();
        assert_eq!(response.status, 401);
        let challenges: Vec<&str> = response.all("www-authenticate").collect();
        assert_eq!(challenges, ["Bearer realm=\"x\"", "Basic"]);
        assert_eq!(response.media_type().as_deref(), Some("application/json"));
        assert_eq!(response.read_all(2).unwrap(), b"{}");
        handle.join().unwrap();
    }

    #[test]
    fn an_idle_timeout_bounds_each_wait_for_bytes_not_the_whole_answer() {
        use std::time::Instant;
        // An answer that comes a piece at a time, for longer in all than
        // the timeout, then one that stops coming.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let head = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n";
            for pieces in [12usize, 1] {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buffer = [0u8; 4096];
                let _ = stream.read(&mut buffer);
                let _ = stream.write_all(head.as_bytes());
                for _ in 0..pieces {
                    let _ = stream.write_all(b"data: x\n\n");
                    let _ = stream.flush();
                    std::thread::sleep(Duration::from_millis(125));
                }
                if pieces == 1 {
                    // Nothing more, and the connection kept open.
                    std::thread::sleep(Duration::from_secs(6));
                }
            }
        });
        let idle = Duration::from_millis(1_000);
        let client = Client::new(Options {
            allow_private: true,
            proxy_from_env: false,
            // An overall deadline this short would cut the first answer.
            timeout: idle,
            idle_timeout: Some(idle),
            ..Options::default()
        })
        .unwrap();
        let url = format!("http://127.0.0.1:{port}/stream");
        let mut response = client.send("POST", &url, &[], Some(b"{}")).unwrap();
        let mut whole = Vec::new();
        response.body().read_to_end(&mut whole).unwrap();
        assert_eq!(
            whole.len(),
            12 * "data: x\n\n".len(),
            "an answer a second and a half long, read whole"
        );
        let started = Instant::now();
        let mut response = client.send("POST", &url, &[], Some(b"{}")).unwrap();
        let mut partial = Vec::new();
        assert!(
            response.body().read_to_end(&mut partial).is_err(),
            "an answer that stops coming is a failure"
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "known within the idle timeout, not when the server gives up"
        );
    }

    #[test]
    fn a_guarded_client_does_not_reach_the_machine_itself() {
        let client = Client::new(Options {
            proxy_from_env: false,
            ..Options::default()
        })
        .unwrap();
        let error = client.get("http://127.0.0.1:9/", 10).unwrap_err();
        assert!(matches!(error, HttpError::Transport(_)), "{error:?}");
        assert!(error.to_string().contains("local or private"), "{error}");
        assert!(matches!(
            client.get("file:///etc/passwd", 10),
            Err(HttpError::Refused(_))
        ));
    }

    #[test]
    fn a_connection_in_the_clear_turns_to_tls_when_asked() {
        // A server that greets in the clear, as IMAP does before STARTTLS,
        // then tells what came next and answers it in the clear.
        let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = server.local_addr().unwrap();
        let (said, heard) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let (mut tcp, _) = server.accept().unwrap();
            tcp.write_all(b"* OK ready\r\n").unwrap();
            let mut next = [0u8; 1];
            let _ = said.send(tcp.read_exact(&mut next).map(|()| next[0]).ok());
            let _ = tcp.write_all(b"* BYE not TLS\r\n");
        });
        let client = Client::new(Options {
            allow_private: true,
            proxy_from_env: false,
            ..Options::default()
        })
        .unwrap();
        let mut plain = client.connect(&format!("imap://{address}")).unwrap();
        assert!(!plain.is_tls());
        assert_eq!(plain.host(), "127.0.0.1");
        let mut greeting = [0u8; 12];
        plain.read_exact(&mut greeting).unwrap();
        assert_eq!(&greeting, b"* OK ready\r\n");
        let mut tls = client.start_tls(plain).unwrap();
        assert!(tls.is_tls());
        // The first write starts a TLS handshake (a record of type 22),
        // which a server speaking in the clear fails.
        let written = tls
            .write_all(b"a1 CAPABILITY\r\n")
            .and_then(|()| tls.flush());
        let mut byte = [0u8; 1];
        assert!(written.is_err() || tls.read(&mut byte).is_err());
        assert_eq!(heard.recv().unwrap(), Some(22));
        assert!(matches!(client.start_tls(tls), Err(HttpError::Refused(_))));
    }

    /// A key made for these tests (`openssl genpkey -algorithm RSA`): it
    /// signs nothing else.
    const TEST_KEY: &str = "-----BEGIN PRIVATE KEY-----\nMIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQC8TD/OCBgIPjSx\nAuFzHFmrJ+NkihIkJAwh7iz8Jid9tKk+YECr47SzgbNAma3z1NdpAemUVo/RWrLt\nKXXlfbdnWW+RlwCDW9iceePOeXIiLvE1gc5u9fFytaGEthzNArd5PtLXInR2YOfr\nhMWI8OHXewnWcm8YmACpw5BfbTGzIRocbmlpjAxr3XBrmzTJ6oQ9I2UNgDPRaOEX\n0uIO/3uq7lCBOeDh4b87LwSaOA/iFDs+OsqiHX0SOq0+cjcp5VaBr5ivdPMw0Bea\niXqyQAS+/Z7WeC/VSP2LPir1x56E7t1uOaQA4dzvvR10WhxQ1qouWX9g1z12OL9K\nWiTG2ln3AgMBAAECggEAUBv4gUU8RDWC4VQiaoDDPWgI/DspoKqdZsoX5WauInL8\nFgeJPBfyRvSFOJSpRzVkN86Se4OgSCdYPEIJYIttvdB1APHHTBuFOn05CBzvC9AA\n2wzQsLB9+pl/kCCSfD4NpM2vkcuc1MhBoxenmq0wyCYWd1/UGBiDH7aN86tXG6fc\nkpWvGhFzlkCQ+R1kNeYElbg8R1RbKx1SZzYd3qybFZ55NK/wgCqiPFGcb3Q42tqD\nscoR5bO/sY8/uqMPk0MmYYNLf8UYp+02jqBcYHYB6dyipSNEynRO0sv8D56o001z\n2/EIjcLNRnpArwuJDloMzJAhAfkpQEHEp7jI79B3QQKBgQDmqP31rJXe3oPlcnGt\ng6QZrW3i9NDzKgMQ1tKYeaLDzI6pl7wR9yq4/tQwFCSxqtsL3qzaFTnFoaYbHVwp\nrMo80Li6IR1lqShvkYBRFgDKMCLWDfPjZJPO4PK/2Gdk5Q92VsBW0V61BPyxGXJ9\nnqrR27lbsWXKLkkDKyoFkZlZpwKBgQDQ+9/v93+EzEdaJhM0gAsUpI/dJQmArCro\nuh6O6k42rqI17n56D1t2VXCxlluRpBE4zyF9c9hyeWw3GrmqfYDRHTx0ywuB4Nh/\nb5qItI5jfZaY8xCcmQXlDkY2unvk6nYGSXfmv7lDMVyzcfLNVtwaE00RlA/uVmWw\n6BqTcOJnMQKBgQC2EqeFCdqztkm/EHuTT9p1OmUHReC6UWtiG3DZUwqcLBhg3pli\nRQzAQscTvJLKe+e0ku02qJKV4mfP93kMnIx3PPrUUdd2q/RzPRNg7lFpSF33QcRG\nttnF7+MudJAJIJb3/rqMzXBIy8r7sO6jImAMj7jdHEcRNUAE9EmTY778+QKBgQCH\nLRS5rFaAeH4CsrQjdls0fAlOWwGdiD+tD45qlOYIescTqXOH0l0v2q4tQHt0ZjJ5\nk4kvE3rC+QVlU1UbEVj8ajPLu/ZyiRSgwXisylV7xyVdQ4PQNy7X7Xa/Obqf5IR5\nJpZ23DaiXtB+hcsuxOtTtn5YS3ROjvXyCv/9tLvNUQKBgHVTMluGaVfsd/VemwMK\nIsiUrdLTcNVvFYhWz3g0hlPWw3QYmgoEbV+v3t/oSmdxyVxQrPnpD2z2ob5PBLDO\nbAuDg06BoUrAfK/9Vy4iVzctRC3Ag5hpR9Rpr0DDMzp6osnnIGDdMayrKqXcpNVz\nlC7ht8tPGw3yR8PC8q6Rm2bn\n-----END PRIVATE KEY-----";
    const TEST_N: &str = "vEw_zggYCD40sQLhcxxZqyfjZIoSJCQMIe4s_CYnfbSpPmBAq-O0s4GzQJmt89TXaQHplFaP0Vqy7Sl15X23Z1lvkZcAg1vYnHnjznlyIi7xNYHObvXxcrWhhLYczQK3eT7S1yJ0dmDn64TFiPDh13sJ1nJvGJgAqcOQX20xsyEaHG5paYwMa91wa5s0yeqEPSNlDYAz0WjhF9LiDv97qu5QgTng4eG_Oy8EmjgP4hQ7PjrKoh19EjqtPnI3KeVWga-Yr3TzMNAXmol6skAEvv2e1ngv1Uj9iz4q9ceehO7dbjmkAOHc770ddFocUNaqLll_YNc9dji_SlokxtpZ9w";
    /// `openssl dgst -sha256 -sign` of `header.payload` with the key.
    const TEST_SIGNATURE: &str = "GQilAVT2bC6vYKlglgfoj6YeC1zpo0WbwVa3smO2NJGyGt-3DWGr0qalsq0OuTKz76CACtHm8s-MUvh9mBjXSU5JDAhGIZucgpFVCGW6g17VKS8tC7pWxhIJ2tocl0rVSDG95WAtRkDHWJzrIN-MLJCX3sX5YqeKSVC9873I-5v2_meWt_yikBs6ZdSLFnRGUMavMkSNkewfNxmjYJ7F4ngAYvAxp1C_MCQKPGbJLpCctIbcLi7EUeJyJYMlX4dD6XDV-dzOzMcTx7FgajKIFuH7Q0veF-QS9f_btcX8y2kraMlEwuutEyc0Ygc_nP6Z-LkgkXEekf8lPQMzC3u1Fw";

    fn from_base64url(text: &str) -> Vec<u8> {
        let value = |c: u8| -> u32 {
            match c {
                b'A'..=b'Z' => u32::from(c - b'A'),
                b'a'..=b'z' => u32::from(c - b'a') + 26,
                b'0'..=b'9' => u32::from(c - b'0') + 52,
                b'-' => 62,
                _ => 63,
            }
        };
        let (mut out, mut acc, mut bits) = (Vec::new(), 0u32, 0);
        for c in text.bytes() {
            acc = (acc << 6) | value(c);
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push((acc >> bits) as u8);
                acc &= (1 << bits) - 1;
            }
        }
        out
    }

    #[test]
    fn rsa_signatures_are_made_and_checked_as_rs256_wants() {
        let n = from_base64url(TEST_N);
        let e = [1, 0, 1];
        let expected = from_base64url(TEST_SIGNATURE);
        // PKCS#1 v1.5 is deterministic: the same as OpenSSL's.
        let signature = rsa_sha256_sign(TEST_KEY, b"header.payload").unwrap();
        assert_eq!(signature, expected);
        assert!(rsa_sha256_verify(&n, &e, b"header.payload", &signature));
        assert!(!rsa_sha256_verify(&n, &e, b"header.payloaD", &signature));
        let mut wrong = signature.clone();
        wrong[0] ^= 1;
        assert!(!rsa_sha256_verify(&n, &e, b"header.payload", &wrong));
        // Leading zeros of the modulus do not change the key.
        let mut padded = vec![0, 0];
        padded.extend(&n);
        assert!(rsa_sha256_verify(
            &padded,
            &e,
            b"header.payload",
            &signature
        ));
        assert!(!rsa_sha256_verify(
            &n[..128],
            &e,
            b"header.payload",
            &signature
        ));
        assert!(matches!(
            rsa_sha256_sign(
                "-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----",
                b"x"
            ),
            Err(HttpError::Config(_))
        ));
        assert_eq!(der_length(0x7f), [0x7f]);
        assert_eq!(der_length(0x100), [0x82, 1, 0]);
        assert_eq!(der_unsigned(&[0x80]), [2, 2, 0, 0x80]);
        assert_eq!(der_unsigned(&[0, 0, 1]), [2, 1, 1]);
    }

    #[test]
    fn the_tls_configuration_offers_the_hybrid_key_exchange_first() {
        let config = tls_config().unwrap();
        let groups: Vec<_> = config
            .crypto_provider()
            .kx_groups
            .iter()
            .map(|g| g.name())
            .collect();
        assert_eq!(groups[0], rustls::NamedGroup::X25519MLKEM768);
    }
}

#[cfg(test)]
mod live {
    use super::*;

    /// A real HTTPS request through whatever this environment provides (a
    /// proxy and its roots included). Needs the network.
    #[test]
    #[ignore = "needs the network"]
    fn fetches_a_page_over_https() {
        let client = Client::new(Options::default()).unwrap();
        let fetched = client
            .get("https://index.crates.io/config.json", 64 * 1024)
            .unwrap();
        assert_eq!(fetched.status, 200);
        assert!(String::from_utf8_lossy(&fetched.body).contains("dl"));
    }
}
