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
    },
    /// The body is longer than the limit.
    #[error("the response is longer than {0} bytes")]
    TooLarge(u64),
    /// Configuration (roots, TLS).
    #[error("configuration: {0}")]
    Config(String),
}

/// How a [`Client`] behaves.
#[derive(Clone, Debug)]
pub struct Options {
    /// Connect timeout.
    pub connect_timeout: Duration,
    /// Overall timeout of one request, body included.
    pub timeout: Duration,
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
}

impl Client {
    /// A client with `options`.
    pub fn new(options: Options) -> Result<Client, HttpError> {
        let tls = Arc::new(PostQuantumTls {
            config: tls_config()?,
        });
        let build = |proxy: Option<(&str, ureq::Proxy)>| {
            let mut allowed = Vec::new();
            let mut builder = ureq::AgentBuilder::new()
                .timeout_connect(options.connect_timeout)
                .timeout(options.timeout)
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
        let proxied = if options.proxy_from_env {
            proxy_from_env()
                .map(|(url, netloc)| -> Result<_, HttpError> {
                    let proxy = ureq::Proxy::new(&url)
                        .map_err(|e| HttpError::Config(format!("proxy {url}: {e}")))?;
                    Ok((build(Some((&netloc, proxy))), NoProxy::from_env()))
                })
                .transpose()?
        } else {
            None
        };
        Ok(Client {
            options,
            direct,
            proxied,
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

    /// One request: `method url` with `headers` and an optional body.
    pub fn request(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, &str)],
        body: Option<&[u8]>,
        max_bytes: u64,
    ) -> Result<Fetched, HttpError> {
        let host = check_url(url)?;
        let agent = match &self.proxied {
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
        };
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
                let mut start = Vec::new();
                let _ = response.into_reader().take(2_048).read_to_end(&mut start);
                return Err(HttpError::Status {
                    status,
                    body: String::from_utf8_lossy(&start).into_owned(),
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
                body: "nope".into()
            })
        );
        handle.join().unwrap();
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
