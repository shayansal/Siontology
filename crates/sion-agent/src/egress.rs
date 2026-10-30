//! Where a skill's scripts may connect, enforced: a filtering proxy
//! outside the sandbox, and a bridge inside it.
//!
//! A skill that names the hosts its scripts reach (`sion-network:
//! api.github.com`) has its scripts run with no network of their own: the
//! sandbox has only a loopback interface. Inside it, the [`bridge`] listens
//! on [`LISTEN`] and hands every connection, through a Unix socket bound
//! into the sandbox, to the [`Egress`] proxy outside, which takes a
//! `CONNECT host:port` (or a plain HTTP request) only to a declared host,
//! on a declared port, at an address that is not private, loopback or
//! link-local. The scripts find the proxy in `HTTPS_PROXY` and the like; a
//! program that ignores those reaches nothing, so the filter fails closed.
//! When the runtime itself must go through a proxy (`HTTPS_PROXY` in its
//! environment), the egress proxy goes through it too.
//!
//! What it does not do is named in the trust model (`skill:declared-hosts`):
//! it filters by the host a script asks for, so a declared host that
//! forwards what it receives, or serves what others put there, takes the
//! script further.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, TcpStream, ToSocketAddrs};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::permissions::ScriptNetwork;

/// Where the bridge listens inside the sandbox.
pub const LISTEN: &str = "127.0.0.1:3128";

/// Where the directory holding the proxy's socket is inside the sandbox.
pub const SOCKET_DIR_INSIDE: &str = "/run/sion-egress";

/// The proxy's socket, in its directory.
pub const SOCKET_NAME: &str = "egress.sock";

/// Where the bridge program is inside the sandbox.
pub const BRIDGE_INSIDE: &str = "/run/sion-bridge";

/// The longest request head the proxy reads.
const MAX_HEAD: usize = 16 << 10;

static PROXIES: AtomicU64 = AtomicU64::new(0);

/// A proxy the runtime's own connections must go through.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Upstream {
    /// Its host.
    pub host: String,
    /// Its port.
    pub port: u16,
    /// `Basic` credentials, base64-encoded, if its URL has a user.
    pub auth: Option<String>,
}

impl Upstream {
    /// The proxy an `http://[user:password@]host:port` URL names.
    pub fn parse(url: &str) -> Option<Upstream> {
        let rest = url.trim().strip_prefix("http://")?;
        let authority = rest.split('/').next()?;
        let (auth, hostport) = match authority.rsplit_once('@') {
            Some((user, hostport)) => (Some(base64(user.as_bytes())), hostport),
            None => (None, authority),
        };
        let (host, port) = match hostport.rsplit_once(':') {
            Some((host, port)) => (host.to_string(), port.parse().ok()?),
            None => (hostport.to_string(), 80),
        };
        (!host.is_empty()).then_some(Upstream { host, port, auth })
    }

    /// The proxy the runtime's environment names (`HTTPS_PROXY`,
    /// `https_proxy`), if any.
    pub fn from_env() -> Option<Upstream> {
        ["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"]
            .iter()
            .find_map(|k| std::env::var(k).ok().and_then(|v| Upstream::parse(&v)))
    }
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// How the egress proxy connects.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// Whether private, loopback and link-local addresses may be reached
    /// (tests only: a stand-in on this machine).
    pub allow_private: bool,
    /// The proxy its own connections go through, if any.
    pub upstream: Option<Upstream>,
}

/// A running egress proxy, stopped when dropped.
#[derive(Debug)]
pub struct Egress {
    dir: PathBuf,
    socket: PathBuf,
    stop: Arc<AtomicBool>,
    log: Arc<Mutex<Vec<String>>>,
    thread: Option<JoinHandle<()>>,
}

impl Egress {
    /// Starts a proxy that lets connections reach only what `allowed`
    /// allows (any public host, the hosts it names, or nothing), on a Unix
    /// socket in a new directory only this user can enter.
    pub fn start(allowed: ScriptNetwork, options: Options) -> Result<Egress, String> {
        let dir = std::env::temp_dir().join(format!(
            "sion-egress-{}-{}",
            std::process::id(),
            PROXIES.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
                .map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let socket = dir.join(SOCKET_NAME);
        let listener =
            UnixListener::bind(&socket).map_err(|e| format!("{}: {e}", socket.display()))?;
        let stop = Arc::new(AtomicBool::new(false));
        let log = Arc::new(Mutex::new(Vec::new()));
        let allowed = Arc::new(allowed);
        let options = Arc::new(options);
        let thread = {
            let stop = stop.clone();
            let log = log.clone();
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(stream) = stream else { continue };
                    let (allowed, options, log) = (allowed.clone(), options.clone(), log.clone());
                    std::thread::spawn(move || serve(stream, &allowed, &options, &log));
                }
            })
        };
        Ok(Egress {
            dir,
            socket,
            stop,
            log,
            thread: Some(thread),
        })
    }

    /// The directory holding the socket, to bind into the sandbox at
    /// [`SOCKET_DIR_INSIDE`].
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The connections asked for so far: `host:port allowed`, or `host:port
    /// refused: why`.
    pub fn log(&self) -> Vec<String> {
        self.log.lock().map(|l| l.clone()).unwrap_or_default()
    }
}

impl Drop for Egress {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake the accept loop so that it sees the flag.
        let _ = UnixStream::connect(&self.socket);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn note(log: &Mutex<Vec<String>>, line: String) {
    if let Ok(mut log) = log.lock() {
        if log.len() < 256 {
            log.push(line);
        }
    }
}

fn refuse(stream: &mut UnixStream, why: &str) {
    let body = format!("{why}\n");
    let _ = write!(
        stream,
        "HTTP/1.1 403 Forbidden\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

/// The request head a client sent: its lines up to the blank one, and any
/// bytes after it already read.
fn read_head(stream: &mut UnixStream) -> Result<(Vec<String>, Vec<u8>), String> {
    let mut reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
    let mut lines = Vec::new();
    let mut total = 0usize;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).map_err(|e| e.to_string())?;
        total += n;
        if n == 0 || total > MAX_HEAD {
            return Err("the request head is cut short or too long".into());
        }
        let line = line.trim_end_matches(['\r', '\n']).to_string();
        if line.is_empty() {
            break;
        }
        lines.push(line);
    }
    let rest = reader.buffer().to_vec();
    Ok((lines, rest))
}

/// Where a request goes: its host and port, and for plain HTTP the head to
/// send on.
struct Target {
    host: String,
    port: u16,
    tunnel: bool,
}

fn target_of(request_line: &str) -> Result<Target, String> {
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().ok_or("no request target")?;
    if method.eq_ignore_ascii_case("CONNECT") {
        let (host, port) = split_host_port(target, 443)?;
        return Ok(Target {
            host,
            port,
            tunnel: true,
        });
    }
    let rest = target
        .strip_prefix("http://")
        .ok_or("only CONNECT and http:// requests go through this proxy")?;
    let authority = rest.split('/').next().unwrap_or_default();
    let (host, port) = split_host_port(authority, 80)?;
    Ok(Target {
        host,
        port,
        tunnel: false,
    })
}

fn split_host_port(text: &str, default: u16) -> Result<(String, u16), String> {
    if let Some(v6) = text.strip_prefix('[') {
        let (host, rest) = v6.split_once(']').ok_or("an IPv6 address without its ]")?;
        let port = match rest.strip_prefix(':') {
            Some(p) => p.parse().map_err(|_| "a port that is not a number")?,
            None => default,
        };
        return Ok((host.to_ascii_lowercase(), port));
    }
    match text.rsplit_once(':') {
        Some((host, port)) => Ok((
            host.to_ascii_lowercase(),
            port.parse().map_err(|_| "a port that is not a number")?,
        )),
        None => Ok((text.to_ascii_lowercase(), default)),
    }
}

/// Connects to `host:port` for a script: through the upstream proxy if
/// there is one, else directly, never to an address that is not public
/// (unless allowed).
fn connect(host: &str, port: u16, options: &Options) -> Result<TcpStream, String> {
    let public_or_allowed = |ip: IpAddr| options.allow_private || sion_http::is_public(ip);
    if let Ok(ip) = host.parse::<IpAddr>() {
        if !public_or_allowed(ip) {
            return Err(format!("{host} is not a public address"));
        }
    }
    let resolved: Option<Vec<std::net::SocketAddr>> =
        (host, port).to_socket_addrs().ok().map(|a| a.collect());
    if let Some(addrs) = &resolved {
        if let Some(bad) = addrs.iter().find(|a| !public_or_allowed(a.ip())) {
            return Err(format!(
                "{host} resolves to {}, which is not public",
                bad.ip()
            ));
        }
    }
    let timeout = Duration::from_secs(15);
    match &options.upstream {
        Some(up) => {
            let addr = (up.host.as_str(), up.port)
                .to_socket_addrs()
                .map_err(|e| format!("the upstream proxy {}: {e}", up.host))?
                .next()
                .ok_or("the upstream proxy has no address")?;
            let mut stream = TcpStream::connect_timeout(&addr, timeout)
                .map_err(|e| format!("the upstream proxy: {e}"))?;
            let auth = up
                .auth
                .as_ref()
                .map(|a| format!("Proxy-Authorization: Basic {a}\r\n"))
                .unwrap_or_default();
            write!(
                stream,
                "CONNECT {host}:{port} HTTP/1.1\r\nHost: {host}:{port}\r\n{auth}\r\n"
            )
            .map_err(|e| e.to_string())?;
            let mut reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
            let mut status = String::new();
            reader.read_line(&mut status).map_err(|e| e.to_string())?;
            if status.split_whitespace().nth(1) != Some("200") {
                return Err(format!("the upstream proxy answered {}", status.trim()));
            }
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).map_err(|e| e.to_string())? == 0
                    || line.trim().is_empty()
                {
                    break;
                }
            }
            Ok(stream)
        }
        None => {
            let addrs = resolved.ok_or_else(|| format!("{host} does not resolve"))?;
            let mut last = format!("{host} has no address");
            for addr in addrs {
                match TcpStream::connect_timeout(&addr, timeout) {
                    Ok(stream) => return Ok(stream),
                    Err(e) => last = format!("{host}:{port}: {e}"),
                }
            }
            Err(last)
        }
    }
}

/// Copies both ways between a client and a server until either closes.
fn splice(client: UnixStream, server: TcpStream) {
    let (Ok(mut client_read), Ok(mut server_write)) = (client.try_clone(), server.try_clone())
    else {
        return;
    };
    let up = std::thread::spawn(move || {
        let _ = std::io::copy(&mut client_read, &mut server_write);
        let _ = server_write.shutdown(std::net::Shutdown::Write);
    });
    let (mut server_read, mut client_write) = (server, client);
    let _ = std::io::copy(&mut server_read, &mut client_write);
    let _ = client_write.shutdown(std::net::Shutdown::Write);
    let _ = up.join();
}

/// Whether `allowed` lets a script reach `host:port`.
fn allows(allowed: &ScriptNetwork, host: &str, port: u16) -> bool {
    match allowed {
        ScriptNetwork::None => false,
        ScriptNetwork::Any => true,
        ScriptNetwork::Hosts(rules) => rules.iter().any(|r| r.allows(host, port)),
    }
}

/// The host a request's `Host` header names, without its port.
fn host_header(lines: &[String]) -> Option<String> {
    lines.iter().skip(1).find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim().eq_ignore_ascii_case("host").then(|| {
            split_host_port(value.trim(), 80)
                .map(|(host, _)| host)
                .unwrap_or_default()
        })
    })
}

fn serve(
    mut stream: UnixStream,
    allowed: &ScriptNetwork,
    options: &Options,
    log: &Mutex<Vec<String>>,
) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let Ok((lines, rest)) = read_head(&mut stream) else {
        return;
    };
    let Some(first) = lines.first() else { return };
    let target = match target_of(first) {
        Ok(target) => target,
        Err(why) => {
            note(log, format!("refused {first:?}: {why}"));
            return refuse(&mut stream, &why);
        }
    };
    let place = format!("{}:{}", target.host, target.port);
    if !allows(allowed, &target.host, target.port) {
        let why = format!("{place} is not among the hosts the skill declares");
        note(log, format!("{place} refused: not a declared host"));
        return refuse(&mut stream, &why);
    }
    // A plain request names its host twice: the server must not be asked,
    // in its Host header, for a site other than the one allowed.
    if !target.tunnel {
        if let Some(named) = host_header(&lines) {
            if named != target.host {
                let why = format!("the request is for {place} but its Host header names {named}");
                note(log, format!("{place} refused: Host header {named}"));
                return refuse(&mut stream, &why);
            }
        }
    }
    let mut server = match connect(&target.host, target.port, options) {
        Ok(server) => server,
        Err(why) => {
            note(log, format!("{place} refused: {why}"));
            return refuse(&mut stream, &why);
        }
    };
    let _ = stream.set_read_timeout(None);
    if target.tunnel {
        if stream
            .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
            .is_err()
        {
            return;
        }
        // Bytes a client sent after its CONNECT, before the answer.
        if !rest.is_empty() && server.write_all(&rest).is_err() {
            return;
        }
    } else {
        // Plain HTTP: the request goes on in origin form when connecting
        // directly, and as it is through an upstream proxy.
        let mut head = lines.clone();
        if options.upstream.is_none() {
            let mut parts = head[0].splitn(3, ' ');
            let method = parts.next().unwrap_or_default().to_string();
            let url = parts.next().unwrap_or_default();
            let version = parts.next().unwrap_or("HTTP/1.1").to_string();
            let after = url.strip_prefix("http://").unwrap_or(url);
            let path = after.find('/').map(|i| &after[i..]).unwrap_or("/");
            head[0] = format!("{method} {path} {version}");
        } else if let Some(auth) = options.upstream.as_ref().and_then(|u| u.auth.clone()) {
            head.push(format!("Proxy-Authorization: Basic {auth}"));
        }
        let mut bytes = head.join("\r\n").into_bytes();
        bytes.extend_from_slice(b"\r\n\r\n");
        bytes.extend_from_slice(&rest);
        if server.write_all(&bytes).is_err() {
            return;
        }
    }
    note(log, format!("{place} allowed"));
    splice(stream, server);
}

/// The variables that send a script's connections to the bridge.
pub fn proxy_env() -> Vec<(String, String)> {
    let url = format!("http://{LISTEN}");
    let mut env = Vec::new();
    for key in [
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "ALL_PROXY",
        "all_proxy",
    ] {
        env.push((key.to_string(), url.clone()));
    }
    env.push(("NO_PROXY".into(), String::new()));
    env.push(("no_proxy".into(), String::new()));
    // Node reads the proxy variables only when told to.
    env.push(("NODE_USE_ENV_PROXY".into(), "1".into()));
    env
}

/// The bridge, inside the sandbox: listens on `listen`, runs `argv`, and
/// hands every connection to the Unix socket `socket` until the program
/// ends. Returns its exit status (125 when the bridge itself fails, 128
/// and the signal when the program was killed).
pub fn bridge(listen: &str, socket: &Path, argv: &[String]) -> i32 {
    let Some((program, args)) = argv.split_first() else {
        eprintln!("sion sandbox bridge: no program to run");
        return 125;
    };
    let listener = match std::net::TcpListener::bind(listen) {
        Ok(listener) => listener,
        Err(e) => {
            eprintln!("sion sandbox bridge: cannot listen on {listen}: {e}");
            return 125;
        }
    };
    let socket = socket.to_path_buf();
    std::thread::spawn(move || {
        for client in listener.incoming().flatten() {
            let socket = socket.clone();
            std::thread::spawn(move || {
                let Ok(proxy) = UnixStream::connect(&socket) else {
                    return;
                };
                let (Ok(mut client_read), Ok(mut proxy_write)) =
                    (client.try_clone(), proxy.try_clone())
                else {
                    return;
                };
                let up = std::thread::spawn(move || {
                    let _ = std::io::copy(&mut client_read, &mut proxy_write);
                    let _ = proxy_write.shutdown(std::net::Shutdown::Write);
                });
                let (mut proxy_read, mut client_write) = (proxy, client);
                let _ = std::io::copy(&mut proxy_read, &mut client_write);
                let _ = client_write.shutdown(std::net::Shutdown::Write);
                let _ = up.join();
            });
        }
    });
    match std::process::Command::new(program).args(args).status() {
        Ok(status) => status.code().unwrap_or_else(|| {
            #[cfg(unix)]
            {
                use std::os::unix::process::ExitStatusExt;
                128 + status.signal().unwrap_or(0)
            }
            #[cfg(not(unix))]
            {
                1
            }
        }),
        Err(e) => {
            eprintln!("sion sandbox bridge: cannot run {program}: {e}");
            127
        }
    }
}

/// The bridge's command line: `<listen> <socket> <program> [args...]`.
pub fn bridge_cli(mut args: impl Iterator<Item = String>) -> i32 {
    let (Some(listen), Some(socket)) = (args.next(), args.next()) else {
        eprintln!("usage: <listen address> <socket> <program> [arguments...]");
        return 125;
    };
    let argv: Vec<String> = args.collect();
    bridge(&listen, Path::new(&socket), &argv)
}

/// Reads all of `stream`; for tests.
#[doc(hidden)]
pub fn read_all(mut stream: impl Read) -> String {
    let mut out = String::new();
    let _ = stream.read_to_string(&mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upstream_proxies_parse_with_and_without_a_user() {
        let up = Upstream::parse("http://proxy.local:3128").unwrap();
        assert_eq!(
            (up.host.as_str(), up.port, up.auth),
            ("proxy.local", 3128, None)
        );
        let up = Upstream::parse("http://u:p@10.0.0.1:8080/").unwrap();
        assert_eq!(up.auth.as_deref(), Some("dTpw"));
        assert!(Upstream::parse("socks5://x:1").is_none());
        assert_eq!(base64(b"user:pass"), "dXNlcjpwYXNz");
    }

    #[test]
    fn targets_are_read_from_connect_and_absolute_requests() {
        let t = target_of("CONNECT api.github.com:443 HTTP/1.1").unwrap();
        assert!(t.tunnel && t.host == "api.github.com" && t.port == 443);
        let t = target_of("GET http://Example.org/x HTTP/1.1").unwrap();
        assert!(!t.tunnel && t.host == "example.org" && t.port == 80);
        let t = target_of("CONNECT [::1]:8443 HTTP/1.1").unwrap();
        assert!(t.host == "::1" && t.port == 8443);
        assert!(target_of("GET /relative HTTP/1.1").is_err());
    }

    fn rules(text: &[&str]) -> ScriptNetwork {
        ScriptNetwork::Hosts(
            text.iter()
                .map(|t| crate::permissions::HostRule::parse(t).unwrap())
                .collect(),
        )
    }

    fn local() -> Options {
        Options {
            allow_private: true,
            upstream: None,
        }
    }

    /// A tiny HTTP server on this machine that answers every request with
    /// `hello`.
    fn hello_server() -> u16 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for mut s in listener.incoming().flatten() {
                let mut buf = [0u8; 1024];
                let _ = s.read(&mut buf);
                let _ = s.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
                );
            }
        });
        port
    }

    #[test]
    fn the_proxy_lets_through_only_declared_hosts() {
        let port = hello_server();
        let egress = Egress::start(rules(&[&format!("127.0.0.1:{port}")]), local()).unwrap();
        let socket = egress.dir().join(SOCKET_NAME);
        let ask = |line: &str| {
            let mut s = UnixStream::connect(&socket).unwrap();
            write!(s, "{line}\r\n\r\n").unwrap();
            read_all(s)
        };
        // A plain request to the declared host goes through, in origin form.
        let plain = ask(&format!(
            "GET http://127.0.0.1:{port}/ HTTP/1.1\r\nHost: 127.0.0.1:{port}"
        ));
        assert!(plain.ends_with("hello"), "{plain}");
        // Not when its Host header asks the server for another site.
        let fronted = ask(&format!(
            "GET http://127.0.0.1:{port}/ HTTP/1.1\r\nHost: other.test"
        ));
        assert!(fronted.starts_with("HTTP/1.1 403"), "{fronted}");
        assert!(
            fronted.contains("Host header names other.test"),
            "{fronted}"
        );
        // So does a tunnel.
        let mut tunnel = UnixStream::connect(&socket).unwrap();
        write!(tunnel, "CONNECT 127.0.0.1:{port} HTTP/1.1\r\n\r\n").unwrap();
        let mut answer = [0u8; 39];
        tunnel.read_exact(&mut answer).unwrap();
        assert_eq!(&answer, b"HTTP/1.1 200 Connection established\r\n\r\n");
        tunnel.write_all(b"GET / HTTP/1.1\r\n\r\n").unwrap();
        assert!(read_all(tunnel).ends_with("hello"));
        // Anything else is refused.
        let refused = ask(&format!("CONNECT other.test:{port} HTTP/1.1"));
        assert!(refused.starts_with("HTTP/1.1 403"), "{refused}");
        assert!(refused.contains("not among the hosts"), "{refused}");
        let wrong_port = ask("CONNECT 127.0.0.1:1 HTTP/1.1");
        assert!(wrong_port.starts_with("HTTP/1.1 403"), "{wrong_port}");
        let relative = ask("GET /x HTTP/1.1");
        assert!(relative.starts_with("HTTP/1.1 403"), "{relative}");
        let log = egress.log().join("\n");
        assert!(log.contains(&format!("127.0.0.1:{port} allowed")), "{log}");
        assert!(
            log.contains(&format!("other.test:{port} refused: not a declared host")),
            "{log}"
        );
    }

    #[test]
    fn any_host_still_means_a_public_one_and_none_means_none() {
        let port = hello_server();
        let target = format!("CONNECT 127.0.0.1:{port} HTTP/1.1\r\n\r\n");
        let ask = |egress: &Egress| {
            let mut s = UnixStream::connect(egress.dir().join(SOCKET_NAME)).unwrap();
            s.write_all(target.as_bytes()).unwrap();
            let mut head = [0u8; 12];
            s.read_exact(&mut head).unwrap();
            let head = String::from_utf8_lossy(&head).into_owned();
            if head.ends_with("200") {
                // Through the tunnel, the server answers a request.
                s.write_all(b"GET / HTTP/1.1\r\n\r\n").unwrap();
            }
            head + &read_all(s)
        };
        let any = Egress::start(ScriptNetwork::Any, Options::default()).unwrap();
        let refused = ask(&any);
        assert!(refused.starts_with("HTTP/1.1 403"), "{refused}");
        assert!(refused.contains("not a public address"), "{refused}");
        let any_here = Egress::start(ScriptNetwork::Any, local()).unwrap();
        assert!(ask(&any_here).starts_with("HTTP/1.1 200"));
        let none = Egress::start(ScriptNetwork::None, local()).unwrap();
        assert!(ask(&none).contains("not among the hosts"));
    }

    #[test]
    fn a_public_address_check_refuses_this_machine_without_permission() {
        let port = hello_server();
        let err = connect("127.0.0.1", port, &Options::default()).unwrap_err();
        assert!(err.contains("not a public address"), "{err}");
        let err = connect("localhost", port, &Options::default()).unwrap_err();
        assert!(err.contains("not public"), "{err}");
        let mut ok = connect(
            "127.0.0.1",
            port,
            &Options {
                allow_private: true,
                upstream: None,
            },
        )
        .unwrap();
        ok.write_all(b"GET / HTTP/1.1\r\n\r\n").unwrap();
        assert!(read_all(ok).ends_with("hello"));
    }

    #[test]
    fn the_bridge_runs_its_program_and_passes_on_its_status() {
        let dir = std::env::temp_dir().join(format!("sion-bridge-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let socket = dir.join("none.sock");
        assert_eq!(
            bridge(
                "127.0.0.1:0",
                &socket,
                &["sh".into(), "-c".into(), "exit 7".into()]
            ),
            7
        );
        assert_eq!(bridge("127.0.0.1:0", &socket, &[]), 125);
        assert_eq!(
            bridge("127.0.0.1:0", &socket, &["/no/such/program".into()]),
            127
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
