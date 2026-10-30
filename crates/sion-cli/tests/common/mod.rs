//! What the channel tests share: a WebSocket stand-in for the services
//! that push events over one (Discord's gateway, Slack's Socket Mode).

#![allow(dead_code)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;

/// A WebSocket stand-in's end: what it sends goes to the bridge, and what
/// the bridge sends it is kept, as JSON.
pub struct Peer {
    socket: sion_http::ws::WebSocket<std::net::TcpStream>,
    kept: Arc<Mutex<Vec<Value>>>,
}

impl Peer {
    pub fn send(&mut self, value: Value) {
        self.socket.send_text(&value.to_string()).unwrap();
    }

    /// Keeps what comes for up to `wait`: false once the bridge has gone.
    pub fn keep(&mut self, wait: Duration) -> bool {
        match self.socket.receive(wait) {
            Ok(Some(sion_http::ws::Message::Text(text))) => {
                self.kept
                    .lock()
                    .unwrap()
                    .push(serde_json::from_str(&text).unwrap_or(Value::Null));
                true
            }
            Ok(_) => true,
            Err(_) => false,
        }
    }

    /// Keeps what comes until `n` messages are kept.
    pub fn expect(&mut self, n: usize) {
        let until = std::time::Instant::now() + Duration::from_secs(20);
        while self.kept.lock().unwrap().len() < n && std::time::Instant::now() < until {
            self.keep(Duration::from_millis(200));
        }
    }
}

/// A WebSocket server for one connection (`ws://…`), which runs `script`
/// and then keeps what the bridge sends until it goes.
pub fn live_stand_in(
    script: impl FnOnce(&mut Peer) + Send + 'static,
) -> (String, Arc<Mutex<Vec<Value>>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let kept = Arc::new(Mutex::new(Vec::new()));
    let keep = kept.clone();
    std::thread::spawn(move || {
        let (tcp, _) = listener.accept().unwrap();
        let (socket, _) = sion_http::ws::WebSocket::accept(tcp, 1 << 20).unwrap();
        let mut peer = Peer { socket, kept: keep };
        script(&mut peer);
        let until = std::time::Instant::now() + Duration::from_secs(60);
        while std::time::Instant::now() < until && peer.keep(Duration::from_secs(1)) {}
    });
    (url, kept)
}

/// What is kept, once there are at least `n` of them.
pub fn wait_for(kept: &Arc<Mutex<Vec<Value>>>, n: usize) -> Vec<Value> {
    let until = std::time::Instant::now() + Duration::from_secs(20);
    while kept.lock().unwrap().len() < n && std::time::Instant::now() < until {
        std::thread::sleep(Duration::from_millis(50));
    }
    kept.lock().unwrap().clone()
}

/// A request an HTTP stand-in was sent.
#[derive(Clone, Debug)]
pub struct Sent {
    pub method: String,
    pub url: String,
    /// Names lower-cased.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Sent {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
}

pub type Kept = Arc<Mutex<Vec<Sent>>>;

/// An HTTP stand-in answering each request with the status and body
/// `answer` gives for it, keeping them all.
pub fn http_stand_in(answer: impl Fn(&Sent) -> (u16, Vec<u8>) + Send + 'static) -> (String, Kept) {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let base = format!("http://{}", server.server_addr());
    let kept: Kept = Arc::new(Mutex::new(Vec::new()));
    let keep = kept.clone();
    std::thread::spawn(move || {
        for mut request in server.incoming_requests() {
            let mut body = Vec::new();
            std::io::Read::read_to_end(request.as_reader(), &mut body).unwrap();
            let sent = Sent {
                method: request.method().as_str().to_string(),
                url: request.url().to_string(),
                headers: request
                    .headers()
                    .iter()
                    .map(|h| {
                        (
                            h.field.as_str().as_str().to_ascii_lowercase(),
                            h.value.as_str().to_string(),
                        )
                    })
                    .collect(),
                body,
            };
            let (status, reply) = answer(&sent);
            keep.lock().unwrap().push(sent);
            let _ = request.respond(tiny_http::Response::from_data(reply).with_status_code(status));
        }
    });
    (base, kept)
}

/// The requests kept for `method` and a URL starting with `url`.
pub fn sent_to(kept: &Kept, method: &str, url: &str) -> Vec<Sent> {
    kept.lock()
        .unwrap()
        .iter()
        .filter(|s| s.method == method && s.url.starts_with(url))
        .cloned()
        .collect()
}

/// The multipart part named `name`, in `body`.
pub fn part<'a>(body: &'a [u8], name: &str) -> Option<&'a [u8]> {
    let marker = format!("name=\"{name}\"");
    let at = body
        .windows(marker.len())
        .position(|w| w == marker.as_bytes())?;
    let rest = &body[at..];
    let start = rest.windows(4).position(|w| w == b"\r\n\r\n")? + 4;
    let end = rest[start..].windows(4).position(|w| w == b"\r\n--")?;
    Some(&rest[start..start + end])
}

/// `body`'s form fields (`a=b&c=d`), decoded.
pub fn form(body: &[u8]) -> Vec<(String, String)> {
    let decode = |text: &str| {
        let bytes = text.replace('+', " ").into_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' && i + 2 < bytes.len() {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap();
                out.push(u8::from_str_radix(hex, 16).unwrap());
                i += 3;
            } else {
                out.push(bytes[i]);
                i += 1;
            }
        }
        String::from_utf8(out).unwrap()
    };
    String::from_utf8_lossy(body)
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(k, v)| (decode(k), decode(v)))
        .collect()
}
