//! WebSocket connections (RFC 6455), for services that push events over
//! one (Discord's gateway, Slack's Socket Mode): as a client, over
//! [`Client::connect`], or as a server over a connection already accepted
//! (for stand-ins in tests).
//!
//! The opening handshake is checked (`Sec-WebSocket-Accept`); a client's
//! frames are masked and a server's must not be; pings are answered;
//! fragmented messages are joined and bounded; text must be UTF-8; no
//! extension is offered or accepted. [`WebSocket::receive`] waits as long as
//! it is told to, and a wait that ends in the middle of a frame loses
//! nothing: what was read is kept for the next call.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use crate::{base64, random, sha1, Client, HttpError, Stream, Target};

/// The GUID the handshake hashes with the key.
const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// The longest handshake head read, in bytes.
const MAX_HEAD: usize = 16 << 10;

/// What a WebSocket runs over: a byte stream whose reads can be given a
/// deadline.
pub trait Transport: Read + Write + Send {
    /// How long a read waits (`None`: as long as it takes).
    fn set_read_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()>;
}

impl Transport for Stream {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()> {
        Stream::set_read_timeout(self, timeout)
    }
}

impl Transport for TcpStream {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()> {
        TcpStream::set_read_timeout(self, timeout)
    }
}

/// A message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    /// Text.
    Text(String),
    /// Bytes.
    Binary(Vec<u8>),
}

/// The request a server accepted: its path and headers (names
/// lower-cased).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Opened {
    /// The path and query.
    pub path: String,
    /// The headers.
    pub headers: Vec<(String, String)>,
}

/// One end of a WebSocket connection.
pub struct WebSocket<S: Transport = Stream> {
    stream: S,
    /// A client masks what it sends; a server does not.
    client: bool,
    /// Bytes read and not yet taken as frames.
    read: Vec<u8>,
    /// A fragmented message being gathered: its opcode and bytes so far.
    partial: Option<(u8, Vec<u8>)>,
    /// The longest message taken, in bytes.
    limit: usize,
    closed: bool,
}

impl<S: Transport> std::fmt::Debug for WebSocket<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebSocket")
            .field("client", &self.client)
            .field("closed", &self.closed)
            .finish_non_exhaustive()
    }
}

fn transport(e: std::io::Error) -> HttpError {
    HttpError::Transport(e.to_string())
}

fn protocol(why: &str) -> HttpError {
    HttpError::Transport(format!("WebSocket: {why}"))
}

/// Reads a head (up to and including the blank line) from `stream`:
/// the head, and the bytes read after it.
fn read_head<S: Read>(stream: &mut S) -> Result<(String, Vec<u8>), HttpError> {
    let mut read = Vec::new();
    loop {
        if let Some(at) = read.windows(4).position(|w| w == b"\r\n\r\n") {
            let rest = read.split_off(at + 4);
            return Ok((String::from_utf8_lossy(&read).into_owned(), rest));
        }
        if read.len() > MAX_HEAD {
            return Err(protocol("the handshake's head is too long"));
        }
        let mut chunk = [0u8; 4096];
        let n = stream.read(&mut chunk).map_err(transport)?;
        if n == 0 {
            return Err(protocol("the connection closed during the handshake"));
        }
        read.extend_from_slice(&chunk[..n]);
    }
}

/// The headers of a head after its first line, names lower-cased.
fn headers_of(head: &str) -> Vec<(String, String)> {
    head.split("\r\n")
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
        .collect()
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.as_str())
}

fn has_token(value: Option<&str>, token: &str) -> bool {
    value.is_some_and(|v| v.split(',').any(|t| t.trim().eq_ignore_ascii_case(token)))
}

/// What the server answers to `key`.
fn accept_of(key: &str) -> String {
    base64(&sha1(format!("{key}{GUID}").as_bytes()))
}

/// A frame: FIN, `opcode`, `payload`, masked with `mask` if given.
fn frame(opcode: u8, payload: &[u8], mask: Option<[u8; 4]>) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 14);
    out.push(0x80 | opcode);
    let masked = if mask.is_some() { 0x80 } else { 0 };
    match payload.len() {
        n if n < 126 => out.push(masked | n as u8),
        n if n <= 0xFFFF => {
            out.push(masked | 126);
            out.extend_from_slice(&(n as u16).to_be_bytes());
        }
        n => {
            out.push(masked | 127);
            out.extend_from_slice(&(n as u64).to_be_bytes());
        }
    }
    match mask {
        Some(key) => {
            out.extend_from_slice(&key);
            out.extend(payload.iter().enumerate().map(|(i, b)| b ^ key[i % 4]));
        }
        None => out.extend_from_slice(payload),
    }
    out
}

/// A frame read from the start of a buffer.
struct Frame {
    fin: bool,
    opcode: u8,
    payload: Vec<u8>,
    /// How many bytes of the buffer it took.
    length: usize,
}

/// The frame at the start of `buffer`, if all of it is there; `masked` is
/// whether frames from the other side must be masked.
fn parse(buffer: &[u8], limit: usize, masked: bool) -> Result<Option<Frame>, HttpError> {
    if buffer.len() < 2 {
        return Ok(None);
    }
    let (first, second) = (buffer[0], buffer[1]);
    if first & 0x70 != 0 {
        return Err(protocol("a frame uses an extension none was agreed for"));
    }
    let fin = first & 0x80 != 0;
    let opcode = first & 0x0F;
    if (second & 0x80 != 0) != masked {
        return Err(protocol(if masked {
            "a client's frame is not masked"
        } else {
            "a server's frame is masked"
        }));
    }
    let (length, mut at) = match second & 0x7F {
        126 if buffer.len() < 4 => return Ok(None),
        126 => (u64::from(u16::from_be_bytes([buffer[2], buffer[3]])), 4),
        127 if buffer.len() < 10 => return Ok(None),
        127 => {
            let mut bytes = [0u8; 8];
            bytes.copy_from_slice(&buffer[2..10]);
            (u64::from_be_bytes(bytes), 10)
        }
        n => (u64::from(n), 2),
    };
    if opcode >= 8 && (!fin || length > 125) {
        return Err(protocol("a control frame is fragmented or too long"));
    }
    if length > limit as u64 {
        return Err(HttpError::TooLarge(limit as u64));
    }
    let key = if masked {
        if buffer.len() < at + 4 {
            return Ok(None);
        }
        let key = [buffer[at], buffer[at + 1], buffer[at + 2], buffer[at + 3]];
        at += 4;
        Some(key)
    } else {
        None
    };
    let length = length as usize;
    if buffer.len() < at + length {
        return Ok(None);
    }
    let mut payload = buffer[at..at + length].to_vec();
    if let Some(key) = key {
        for (i, byte) in payload.iter_mut().enumerate() {
            *byte ^= key[i % 4];
        }
    }
    Ok(Some(Frame {
        fin,
        opcode,
        payload,
        length: at + length,
    }))
}

fn message(opcode: u8, payload: Vec<u8>) -> Result<Message, HttpError> {
    if opcode == 1 {
        String::from_utf8(payload)
            .map(Message::Text)
            .map_err(|_| protocol("a text message is not UTF-8"))
    } else {
        Ok(Message::Binary(payload))
    }
}

impl WebSocket<Stream> {
    /// Opens a connection to `url` (`ws`/`wss`, or `http`/`https`) with
    /// `client`, sending `headers` with the handshake, taking messages of
    /// at most `limit` bytes.
    pub fn connect(
        client: &Client,
        url: &str,
        headers: &[(&str, &str)],
        limit: usize,
    ) -> Result<WebSocket<Stream>, HttpError> {
        let target = Target::parse(url)?;
        let mut stream = client.connect(url)?;
        let mut nonce = [0u8; 16];
        random(&mut nonce)?;
        let key = base64(&nonce);
        let mut request = format!(
            "GET {} HTTP/1.1\r\nHost: {}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\nUser-Agent: {}\r\n",
            target.path,
            target.host_header(),
            client.options().user_agent
        );
        for (name, value) in headers {
            if [name, value].iter().any(|s| s.contains(['\r', '\n'])) {
                return Err(HttpError::Refused("a header holds a line break".into()));
            }
            request.push_str(&format!("{name}: {value}\r\n"));
        }
        request.push_str("\r\n");
        stream.write_all(request.as_bytes()).map_err(transport)?;
        stream.flush().map_err(transport)?;
        let (head, rest) = read_head(&mut stream)?;
        let status: u16 = head
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| protocol("the server's answer is not HTTP"))?;
        if status != 101 {
            return Err(HttpError::Status {
                status,
                body: "the server did not open a WebSocket".into(),
            });
        }
        let headers = headers_of(&head);
        if !has_token(header(&headers, "upgrade"), "websocket")
            || !has_token(header(&headers, "connection"), "upgrade")
            || header(&headers, "sec-websocket-accept") != Some(accept_of(&key).as_str())
        {
            return Err(protocol("the server's handshake does not answer ours"));
        }
        if header(&headers, "sec-websocket-extensions").is_some() {
            return Err(protocol("the server chose an extension none was offered"));
        }
        Ok(WebSocket {
            stream,
            client: true,
            read: rest,
            partial: None,
            limit,
            closed: false,
        })
    }
}

impl<S: Transport> WebSocket<S> {
    /// Accepts a connection whose request is still to be read, as a
    /// server: the request's path and headers, and the connection.
    pub fn accept(mut stream: S, limit: usize) -> Result<(WebSocket<S>, Opened), HttpError> {
        let (head, rest) = read_head(&mut stream)?;
        let path = head
            .split_whitespace()
            .nth(1)
            .ok_or_else(|| protocol("the request is not HTTP"))?
            .to_string();
        let headers = headers_of(&head);
        let key = header(&headers, "sec-websocket-key")
            .filter(|_| has_token(header(&headers, "upgrade"), "websocket"))
            .filter(|_| header(&headers, "sec-websocket-version") == Some("13"))
            .ok_or_else(|| protocol("the request does not open a WebSocket"))?
            .to_string();
        let answer = format!(
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
            accept_of(&key)
        );
        stream.write_all(answer.as_bytes()).map_err(transport)?;
        stream.flush().map_err(transport)?;
        Ok((
            WebSocket {
                stream,
                client: false,
                read: rest,
                partial: None,
                limit,
                closed: false,
            },
            Opened { path, headers },
        ))
    }

    fn send_frame(&mut self, opcode: u8, payload: &[u8]) -> Result<(), HttpError> {
        let mask = if self.client {
            let mut key = [0u8; 4];
            random(&mut key)?;
            Some(key)
        } else {
            None
        };
        self.stream
            .write_all(&frame(opcode, payload, mask))
            .map_err(transport)?;
        self.stream.flush().map_err(transport)
    }

    fn check_open(&self) -> Result<(), HttpError> {
        if self.closed {
            return Err(HttpError::Closed {
                code: 1006,
                reason: "this side closed it".into(),
            });
        }
        Ok(())
    }

    /// Sends `text`.
    pub fn send_text(&mut self, text: &str) -> Result<(), HttpError> {
        self.check_open()?;
        self.send_frame(1, text.as_bytes())
    }

    /// Sends `bytes`.
    pub fn send_binary(&mut self, bytes: &[u8]) -> Result<(), HttpError> {
        self.check_open()?;
        self.send_frame(2, bytes)
    }

    /// Sends a ping.
    pub fn ping(&mut self, payload: &[u8]) -> Result<(), HttpError> {
        self.check_open()?;
        self.send_frame(9, &payload[..payload.len().min(125)])
    }

    /// Closes the connection with `code` and `reason` (the other side is
    /// not waited for).
    pub fn close(&mut self, code: u16, reason: &str) {
        if self.closed {
            return;
        }
        let mut payload = code.to_be_bytes().to_vec();
        let mut end = reason.len().min(123);
        while !reason.is_char_boundary(end) {
            end -= 1;
        }
        payload.extend_from_slice(&reason.as_bytes()[..end]);
        let _ = self.send_frame(8, &payload);
        self.closed = true;
    }

    /// The next message, waiting up to `wait` for it: `None` if none came
    /// in time. Pings are answered on the way; when the other side closes
    /// the connection, the close is answered and given as
    /// [`HttpError::Closed`].
    pub fn receive(&mut self, wait: Duration) -> Result<Option<Message>, HttpError> {
        self.check_open()?;
        let deadline = Instant::now() + wait;
        loop {
            while let Some(frame) = parse(&self.read, self.limit, !self.client)? {
                self.read.drain(..frame.length);
                match frame.opcode {
                    9 => self.send_frame(10, &frame.payload)?,
                    10 => {}
                    8 => {
                        let (code, reason) = match frame.payload.len() {
                            0 => (1005, String::new()),
                            1 => return Err(protocol("a close frame of one byte")),
                            _ => (
                                u16::from_be_bytes([frame.payload[0], frame.payload[1]]),
                                String::from_utf8_lossy(&frame.payload[2..]).into_owned(),
                            ),
                        };
                        let echo = frame.payload[..frame.payload.len().min(2)].to_vec();
                        let _ = self.send_frame(8, &echo);
                        self.closed = true;
                        return Err(HttpError::Closed { code, reason });
                    }
                    1 | 2 => {
                        if self.partial.is_some() {
                            return Err(protocol("a message began before the last one ended"));
                        }
                        if frame.fin {
                            return message(frame.opcode, frame.payload).map(Some);
                        }
                        self.partial = Some((frame.opcode, frame.payload));
                    }
                    0 => {
                        let (opcode, mut gathered) = self
                            .partial
                            .take()
                            .ok_or_else(|| protocol("a continuation of no message"))?;
                        gathered.extend_from_slice(&frame.payload);
                        if gathered.len() > self.limit {
                            return Err(HttpError::TooLarge(self.limit as u64));
                        }
                        if frame.fin {
                            return message(opcode, gathered).map(Some);
                        }
                        self.partial = Some((opcode, gathered));
                    }
                    _ => return Err(protocol("a frame of an unknown kind")),
                }
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(None);
            }
            self.stream
                .set_read_timeout(Some(left.max(Duration::from_millis(1))))
                .map_err(transport)?;
            let mut chunk = [0u8; 16 << 10];
            match self.stream.read(&mut chunk) {
                Ok(0) => {
                    self.closed = true;
                    return Err(HttpError::Closed {
                        code: 1006,
                        reason: "the connection ended without a close".into(),
                    });
                }
                Ok(n) => self.read.extend_from_slice(&chunk[..n]),
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(e) => return Err(transport(e)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Options;

    fn local() -> Client {
        Client::new(Options {
            allow_private: true,
            proxy_from_env: false,
            timeout: Duration::from_secs(5),
            ..Options::default()
        })
        .unwrap()
    }

    /// A server that accepts one connection and runs `script` on it.
    fn serve(
        script: impl FnOnce(WebSocket<TcpStream>, Opened) + Send + 'static,
    ) -> (String, std::thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("ws://{}/gateway?v=10", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let (tcp, _) = listener.accept().unwrap();
            let (socket, opened) = WebSocket::accept(tcp, 1 << 20).unwrap();
            script(socket, opened);
        });
        (url, handle)
    }

    #[test]
    fn messages_go_both_ways_and_pings_are_answered() {
        let (url, server) = serve(|mut socket, opened| {
            assert_eq!(opened.path, "/gateway?v=10");
            assert_eq!(header(&opened.headers, "authorization"), Some("Bot x"));
            socket.send_text("hello").unwrap();
            assert_eq!(
                socket.receive(Duration::from_secs(5)).unwrap(),
                Some(Message::Text("identify".into()))
            );
            // A ping is answered with a pong, which the server sees as no
            // message.
            socket.ping(b"are you there").unwrap();
            assert_eq!(socket.receive(Duration::from_millis(300)).unwrap(), None);
            socket.send_binary(&[0, 1, 2]).unwrap();
            socket.close(4004, "authentication failed");
        });
        let mut client =
            WebSocket::connect(&local(), &url, &[("Authorization", "Bot x")], 1 << 20).unwrap();
        assert_eq!(
            client.receive(Duration::from_secs(5)).unwrap(),
            Some(Message::Text("hello".into()))
        );
        client.send_text("identify").unwrap();
        assert_eq!(
            client.receive(Duration::from_secs(5)).unwrap(),
            Some(Message::Binary(vec![0, 1, 2]))
        );
        assert_eq!(
            client.receive(Duration::from_secs(5)),
            Err(HttpError::Closed {
                code: 4004,
                reason: "authentication failed".into()
            })
        );
        assert!(client.send_text("again").is_err());
        server.join().unwrap();
    }

    #[test]
    fn a_wait_that_ends_mid_frame_loses_nothing_and_fragments_are_joined() {
        let (url, server) = serve(|mut socket, _| {
            // Half a frame, a pause longer than the client waits, the rest.
            let whole = frame(1, "a long-awaited message".as_bytes(), None);
            socket.stream.write_all(&whole[..5]).unwrap();
            std::thread::sleep(Duration::from_millis(400));
            socket.stream.write_all(&whole[5..]).unwrap();
            // A message in three fragments, a ping between two of them.
            let mut first = frame(1, b"one ", None);
            first[0] &= 0x7F;
            let mut second = frame(0, b"two ", None);
            second[0] &= 0x7F;
            socket.stream.write_all(&first).unwrap();
            socket.stream.write_all(&frame(9, b"", None)).unwrap();
            socket.stream.write_all(&second).unwrap();
            socket.stream.write_all(&frame(0, b"three", None)).unwrap();
            // Too long a message.
            socket.send_text(&"x".repeat(2000)).unwrap();
            let _ = socket.receive(Duration::from_secs(2));
        });
        let mut client = WebSocket::connect(&local(), &url, &[], 1000).unwrap();
        assert_eq!(client.receive(Duration::from_millis(100)).unwrap(), None);
        assert_eq!(
            client.receive(Duration::from_secs(5)).unwrap(),
            Some(Message::Text("a long-awaited message".into()))
        );
        assert_eq!(
            client.receive(Duration::from_secs(5)).unwrap(),
            Some(Message::Text("one two three".into()))
        );
        assert_eq!(
            client.receive(Duration::from_secs(5)),
            Err(HttpError::TooLarge(1000))
        );
        drop(client);
        server.join().unwrap();
    }

    #[test]
    fn a_handshake_that_does_not_answer_is_refused() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("ws://{}/", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            for answer in [
                "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: bm90IHRoZSBhbnN3ZXI=\r\n\r\n",
                "HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n",
            ] {
                let (mut tcp, _) = listener.accept().unwrap();
                let _ = read_head(&mut tcp).unwrap();
                tcp.write_all(answer.as_bytes()).unwrap();
            }
        });
        let wrong = WebSocket::connect(&local(), &url, &[], 1 << 10).unwrap_err();
        assert!(wrong.to_string().contains("does not answer"), "{wrong}");
        let refused = WebSocket::connect(&local(), &url, &[], 1 << 10).unwrap_err();
        assert!(
            matches!(refused, HttpError::Status { status: 401, .. }),
            "{refused}"
        );
        server.join().unwrap();
        // A guarded client does not reach this machine at all.
        let guarded = Client::new(Options {
            proxy_from_env: false,
            ..Options::default()
        })
        .unwrap();
        let local_only =
            WebSocket::connect(&guarded, "ws://127.0.0.1:9/", &[], 1 << 10).unwrap_err();
        assert!(
            local_only.to_string().contains("local or private"),
            "{local_only}"
        );
        assert!(WebSocket::connect(&guarded, "ftp://example.org/", &[], 1 << 10).is_err());
    }

    #[test]
    fn targets_are_read_from_urls() {
        let t = Target::parse("wss://Gateway.Discord.gg/?v=10&encoding=json").unwrap();
        assert_eq!(
            (t.host.as_str(), t.port, t.tls, t.path.as_str()),
            ("gateway.discord.gg", 443, true, "/?v=10&encoding=json")
        );
        assert_eq!(t.host_header(), "gateway.discord.gg");
        let t = Target::parse("ws://[::1]:8080").unwrap();
        assert_eq!((t.netloc().as_str(), t.path.as_str()), ("[::1]:8080", "/"));
        assert_eq!(t.host_header(), "[::1]:8080");
        assert!(Target::parse("ws://user:pw@example.org/").is_err());
        let irc = Target::parse("ircs://irc.libera.chat").unwrap();
        assert_eq!((irc.port, irc.tls), (6697, true));
        assert_eq!(Target::parse("irc://127.0.0.1:6667").unwrap().port, 6667);
        assert!(Target::parse("ws://example.org:x/").is_err());
        assert_eq!(
            crate::proxy_authorization("http://us%40er:p%3Ass@127.0.0.1:3128").as_deref(),
            Some("Basic dXNAZXI6cDpzcw==")
        );
        assert_eq!(crate::proxy_authorization("http://127.0.0.1:3128"), None);
    }
}

#[cfg(test)]
mod live {
    use super::*;
    use crate::Options;

    /// Through whatever this environment provides (a proxy and its roots
    /// included): an echo server, and Discord's gateway, which says hello
    /// before anything is sent to it. Needs the network.
    #[test]
    #[ignore = "needs the network"]
    fn real_servers_answer_over_tls() {
        let client = Client::new(Options::default()).unwrap();
        let mut echo =
            WebSocket::connect(&client, "wss://echo.websocket.org", &[], 1 << 20).unwrap();
        echo.send_text("sion-ws-check").unwrap();
        let echoed = (0..5).any(|_| {
            echo.receive(Duration::from_secs(10)).unwrap()
                == Some(Message::Text("sion-ws-check".into()))
        });
        assert!(echoed);
        echo.close(1000, "done");
        let mut gateway = WebSocket::connect(
            &client,
            "wss://gateway.discord.gg/?v=10&encoding=json",
            &[],
            1 << 20,
        )
        .unwrap();
        let Some(Message::Text(hello)) = gateway.receive(Duration::from_secs(10)).unwrap() else {
            panic!("no hello");
        };
        assert!(
            hello.contains("\"op\":10") && hello.contains("heartbeat_interval"),
            "{hello}"
        );
        gateway.close(1000, "done");
    }
}
