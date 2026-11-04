//! A browser's page handed to a person ([`crate::browser::hand_over`]): a
//! page of its own at an address of this machine's, made for the hand-over
//! and known only by the token in its path, shows what the browser shows,
//! a picture a moment old, and passes the person's clicks, text, keys and
//! scrolling back to it. Nothing else is served: a request without the
//! token, or for another host than the address itself (a page elsewhere
//! that points a name of its own at this machine), is refused. A request is
//! bounded in size and in time, and a few are served at once.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

/// What the person's page asks of the browser.
#[derive(Debug)]
pub enum Asked {
    /// A picture of the page now (JPEG), sent back.
    View(mpsc::Sender<Option<Vec<u8>>>),
    /// A click at a point, in the page's CSS pixels.
    Click(f64, f64),
    /// Text typed into what has the focus.
    Text(String),
    /// A key pressed.
    Key(String),
    /// A scroll of about a screen, down (`true`) or up.
    Scroll(bool),
}

/// The keys the person's page may press.
pub const KEYS: [&str; 8] = [
    "Enter",
    "Tab",
    "Backspace",
    "Escape",
    "ArrowUp",
    "ArrowDown",
    "ArrowLeft",
    "ArrowRight",
];

/// The most bytes of what the person's page sends.
const MAX_BODY: usize = 4_096;

/// The most bytes of a request's line and headers.
const MAX_HEAD: usize = 8_192;

/// The most headers a request may have.
const MAX_HEADERS: usize = 32;

/// The most requests served at once; more are turned away.
const MAX_SERVING: usize = 8;

/// How long a request may take to arrive, whole.
const REQUEST_SECS: u64 = 10;

/// A hand-over under way: its address, and what the person asks.
#[derive(Debug)]
pub struct Takeover {
    /// Where the person sees the page: `http://127.0.0.1:<port>/<token>`.
    pub url: String,
    asked: mpsc::Receiver<Asked>,
    stop: Arc<AtomicBool>,
}

/// A request being served, counted while it is.
struct Serving(Arc<AtomicUsize>);

impl Drop for Serving {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Takeover {
    /// Serves the hand-over of a page `size` CSS pixels big.
    pub fn start(size: (u32, u32)) -> Result<Takeover, String> {
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let mut bytes = [0u8; 16];
        sion_http::random(&mut bytes).map_err(|e| e.to_string())?;
        let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let url = format!("http://127.0.0.1:{port}/{token}");
        let (sender, asked) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let serving = Arc::new(AtomicUsize::new(0));
        std::thread::Builder::new()
            .name("sion-takeover".into())
            .spawn(move || {
                while !stopped.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            if serving.fetch_add(1, Ordering::SeqCst) >= MAX_SERVING {
                                serving.fetch_sub(1, Ordering::SeqCst);
                                continue;
                            }
                            let counted = Serving(serving.clone());
                            let (sender, token) = (sender.clone(), token.clone());
                            // A thread that cannot be made drops the
                            // connection, and its count with it.
                            let _ = std::thread::Builder::new().spawn(move || {
                                let _counted = counted;
                                let _ = serve(stream, &token, port, size, &sender);
                            });
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(30));
                        }
                        // Out of descriptors, or a connection gone before
                        // it was taken: a moment, and on.
                        Err(_) => std::thread::sleep(Duration::from_millis(200)),
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Takeover { url, asked, stop })
    }

    /// What the person asks next, waiting at most `wait`.
    pub fn next(&self, wait: Duration) -> Option<Asked> {
        self.asked.recv_timeout(wait).ok()
    }
}

impl Drop for Takeover {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn respond(stream: &mut TcpStream, status: &str, kind: &str, body: &[u8]) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\n\
         X-Frame-Options: DENY\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: \
         default-src 'none'; img-src 'self'; connect-src 'self'; script-src 'unsafe-inline'; \
         style-src 'unsafe-inline'\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)?;
    stream.flush()
}

/// The person's page: the browser's page as a picture, refreshed, its
/// clicks, keys and scrolling sent back.
fn page(size: (u32, u32)) -> String {
    let (width, height) = size;
    let keys: String = KEYS
        .iter()
        .map(|key| {
            let shown = key.strip_prefix("Arrow").unwrap_or(key);
            format!("<button data-key=\"{key}\">{shown}</button>")
        })
        .collect();
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><title>The agent's browser</title>
<style>body{{margin:0;font:14px system-ui,sans-serif;background:#111;color:#eee}}
#bar{{padding:8px;display:flex;gap:6px;flex-wrap:wrap;align-items:center}}
img{{display:block;max-width:100%;cursor:crosshair}}</style></head><body>
<div id="bar"><span>Act on the page below; reply where the agent asked you when you are done.</span>
<input id="text" placeholder="Text to type" autocomplete="off" spellcheck="false"><button id="type">Type</button>
{keys}<button data-scroll="up">Scroll up</button><button data-scroll="down">Scroll down</button></div>
<img id="view" alt="the agent's browser" width="{width}" height="{height}">
<script>
const base = location.pathname, view = document.getElementById('view');
const send = (what, body) => fetch(base + '/' + what, {{method: 'POST', body}});
const refresh = () => {{ view.src = base + '/view?' + Date.now(); }};
view.onload = () => setTimeout(refresh, 400);
view.onerror = () => setTimeout(refresh, 1500);
view.addEventListener('click', (e) => {{
  const r = view.getBoundingClientRect();
  send('click', ((e.clientX - r.left) * {width} / r.width) + ' ' + ((e.clientY - r.top) * {height} / r.height));
  setTimeout(refresh, 300);
}});
let wheeled = 0;
view.addEventListener('wheel', (e) => {{
  e.preventDefault();
  if (Date.now() - wheeled < 400) return;
  wheeled = Date.now();
  send('scroll', e.deltaY > 0 ? 'down' : 'up');
}}, {{passive: false}});
const typed = () => {{
  const text = document.getElementById('text');
  send('text', text.value); text.value = '';
}};
document.getElementById('type').onclick = typed;
document.getElementById('text').addEventListener('keydown', (e) => {{ if (e.key === 'Enter') typed(); }});
document.querySelectorAll('[data-key]').forEach((b) => {{ b.onclick = () => send('key', b.dataset.key); }});
document.querySelectorAll('[data-scroll]').forEach((b) => {{ b.onclick = () => send('scroll', b.dataset.scroll); }});
refresh();
</script></body></html>"#
    )
}

/// A reader that stops at its deadline: a request that comes too slowly
/// is not waited for.
struct Timed<R> {
    inner: R,
    deadline: Instant,
}

impl<R: Read> Read for Timed<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if Instant::now() >= self.deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "the request came too slowly",
            ));
        }
        self.inner.read(buf)
    }
}

fn serve(
    mut stream: TcpStream,
    token: &str,
    port: u16,
    size: (u32, u32),
    asked: &mpsc::Sender<Asked>,
) -> std::io::Result<()> {
    stream.set_nonblocking(false)?;
    // Each read waits a little; the whole request, REQUEST_SECS.
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(REQUEST_SECS)))?;
    let timed = Timed {
        inner: stream.try_clone()?,
        deadline: Instant::now() + Duration::from_secs(REQUEST_SECS),
    };
    let mut reader = BufReader::new(timed.take((MAX_HEAD + MAX_BODY) as u64));
    let mut head = 0usize;
    let mut line = String::new();
    head += reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let (method, path) = (
        parts.next().unwrap_or_default().to_string(),
        parts.next().unwrap_or_default().to_string(),
    );
    let mut host = String::new();
    let mut length = 0usize;
    let mut ended = false;
    for _ in 0..MAX_HEADERS {
        let mut header = String::new();
        let read = reader.read_line(&mut header)?;
        head += read;
        if head > MAX_HEAD {
            break;
        }
        if read == 0 || header.trim().is_empty() {
            ended = true;
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            match name.trim().to_ascii_lowercase().as_str() {
                "host" => host = value.trim().to_string(),
                "content-length" => length = value.trim().parse().unwrap_or(usize::MAX),
                _ => {}
            }
        }
    }
    if !ended || !line.ends_with('\n') {
        return respond(
            &mut stream,
            "431 Request Header Fields Too Large",
            "text/plain",
            b"too long",
        );
    }
    // Only this address, by its own name.
    if host != format!("127.0.0.1:{port}") && host != format!("localhost:{port}") {
        return respond(&mut stream, "403 Forbidden", "text/plain", b"not here");
    }
    if length > MAX_BODY {
        return respond(
            &mut stream,
            "413 Payload Too Large",
            "text/plain",
            b"too long",
        );
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body)?;
    let body = String::from_utf8_lossy(&body).to_string();
    let path = path.split('?').next().unwrap_or_default();
    let Some(rest) = path.strip_prefix('/').and_then(|p| p.strip_prefix(token)) else {
        return respond(&mut stream, "404 Not Found", "text/plain", b"");
    };
    match (method.as_str(), rest) {
        ("GET", "") => respond(
            &mut stream,
            "200 OK",
            "text/html; charset=utf-8",
            page(size).as_bytes(),
        ),
        ("GET", "/view") => {
            let (sent, picture) = mpsc::channel();
            if asked.send(Asked::View(sent)).is_err() {
                return respond(&mut stream, "410 Gone", "text/plain", b"over");
            }
            match picture.recv_timeout(Duration::from_secs(5)) {
                Ok(Some(jpeg)) => respond(&mut stream, "200 OK", "image/jpeg", &jpeg),
                _ => respond(
                    &mut stream,
                    "503 Service Unavailable",
                    "text/plain",
                    b"no picture",
                ),
            }
        }
        ("POST", "/click") => {
            let mut point = body
                .split_whitespace()
                .filter_map(|n| n.parse::<f64>().ok());
            match (point.next(), point.next()) {
                (Some(x), Some(y)) if x.is_finite() && y.is_finite() => {
                    let _ = asked.send(Asked::Click(x, y));
                    respond(&mut stream, "204 No Content", "text/plain", b"")
                }
                _ => respond(&mut stream, "400 Bad Request", "text/plain", b"x y"),
            }
        }
        ("POST", "/text") => {
            let _ = asked.send(Asked::Text(body));
            respond(&mut stream, "204 No Content", "text/plain", b"")
        }
        ("POST", "/key") if KEYS.contains(&body.trim()) => {
            let _ = asked.send(Asked::Key(body.trim().to_string()));
            respond(&mut stream, "204 No Content", "text/plain", b"")
        }
        ("POST", "/scroll") if matches!(body.trim(), "down" | "up") => {
            let _ = asked.send(Asked::Scroll(body.trim() == "down"));
            respond(&mut stream, "204 No Content", "text/plain", b"")
        }
        _ => respond(&mut stream, "404 Not Found", "text/plain", b""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask(port: u16, host: &str, request: &str) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let request = request.replace("{host}", host);
        stream.write_all(request.as_bytes()).unwrap();
        // A picture is not text: read bytes.
        let mut answer = Vec::new();
        let _ = stream.read_to_end(&mut answer);
        String::from_utf8_lossy(&answer).into_owned()
    }

    #[test]
    fn a_person_sees_the_page_and_acts_on_it_by_its_token_only() {
        let takeover = Takeover::start((800, 600)).unwrap();
        let (port, token) = {
            let rest = takeover.url.trim_start_matches("http://127.0.0.1:");
            let (port, token) = rest.split_once('/').unwrap();
            (port.parse::<u16>().unwrap(), token.to_string())
        };
        let host = format!("127.0.0.1:{port}");
        let page = ask(
            port,
            &host,
            &format!("GET /{token} HTTP/1.1\r\nHost: {{host}}\r\n\r\n"),
        );
        assert!(page.starts_with("HTTP/1.1 200 OK"), "{page}");
        assert!(page.contains("The agent's browser") && page.contains("width=\"800\""));
        // A click, text and a key reach the browser.
        let clicked = ask(
            port,
            &host,
            &format!("POST /{token}/click HTTP/1.1\r\nHost: {{host}}\r\nContent-Length: 8\r\n\r\n120 45.5"),
        );
        assert!(clicked.starts_with("HTTP/1.1 204"), "{clicked}");
        assert!(
            matches!(takeover.next(Duration::from_secs(2)), Some(Asked::Click(x, y)) if x == 120.0 && y == 45.5)
        );
        ask(
            port,
            &host,
            &format!(
                "POST /{token}/text HTTP/1.1\r\nHost: {{host}}\r\nContent-Length: 6\r\n\r\n123456"
            ),
        );
        assert!(
            matches!(takeover.next(Duration::from_secs(2)), Some(Asked::Text(t)) if t == "123456")
        );
        ask(
            port,
            &host,
            &format!(
                "POST /{token}/key HTTP/1.1\r\nHost: {{host}}\r\nContent-Length: 5\r\n\r\nEnter"
            ),
        );
        assert!(
            matches!(takeover.next(Duration::from_secs(2)), Some(Asked::Key(k)) if k == "Enter")
        );
        let refused = ask(
            port,
            &host,
            &format!(
                "POST /{token}/key HTTP/1.1\r\nHost: {{host}}\r\nContent-Length: 6\r\n\r\nDelete"
            ),
        );
        assert!(refused.starts_with("HTTP/1.1 404"), "{refused}");
        // A picture of the page, as the browser gives it.
        let viewer = std::thread::spawn(move || {
            ask(
                port,
                &host,
                &format!("GET /{token}/view?1 HTTP/1.1\r\nHost: {{host}}\r\n\r\n"),
            )
        });
        match takeover.next(Duration::from_secs(2)) {
            Some(Asked::View(sent)) => sent.send(Some(b"\xff\xd8jpeg".to_vec())).unwrap(),
            other => panic!("{other:?}"),
        }
        let viewed = viewer.join().unwrap();
        assert!(
            viewed.starts_with("HTTP/1.1 200 OK") && viewed.contains("image/jpeg"),
            "{viewed}"
        );
        // Without the token, or by another name: refused.
        let host = format!("127.0.0.1:{port}");
        assert!(
            ask(port, &host, "GET /wrong HTTP/1.1\r\nHost: {host}\r\n\r\n")
                .starts_with("HTTP/1.1 404")
        );
        let token = takeover.url.rsplit('/').next().unwrap().to_string();
        assert!(ask(
            port,
            "evil.example:80",
            &format!("GET /{token} HTTP/1.1\r\nHost: {{host}}\r\n\r\n")
        )
        .starts_with("HTTP/1.1 403"));
        // Scrolling reaches the browser too.
        let scrolled = ask(
            port,
            &host,
            &format!(
                "POST /{token}/scroll HTTP/1.1\r\nHost: {{host}}\r\nContent-Length: 4\r\n\r\ndown"
            ),
        );
        assert!(scrolled.starts_with("HTTP/1.1 204"), "{scrolled}");
        assert!(matches!(
            takeover.next(Duration::from_secs(2)),
            Some(Asked::Scroll(true))
        ));
        // A head too long, or with too many headers, is not read on.
        let long = format!(
            "GET /{token} HTTP/1.1\r\nHost: {{host}}\r\nX-Long: {}\r\n\r\n",
            "a".repeat(MAX_HEAD)
        );
        assert!(ask(port, &host, &long).starts_with("HTTP/1.1 431"));
        let many = format!(
            "GET /{token} HTTP/1.1\r\n{}Host: {{host}}\r\n\r\n",
            "X-A: b\r\n".repeat(MAX_HEADERS + 1)
        );
        assert!(ask(port, &host, &many).starts_with("HTTP/1.1 431"));
    }

    #[test]
    fn a_request_that_comes_too_slowly_is_not_waited_for() {
        let takeover = Takeover::start((800, 600)).unwrap();
        let port: u16 = takeover
            .url
            .trim_start_matches("http://127.0.0.1:")
            .split('/')
            .next()
            .unwrap()
            .parse()
            .unwrap();
        // Half a request, and then nothing: the connection is closed
        // within the request's time, not held.
        let started = Instant::now();
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream.write_all(b"GET / HTTP/1.1\r\nHo").unwrap();
        let mut answer = Vec::new();
        let _ = stream.read_to_end(&mut answer);
        assert!(started.elapsed() < Duration::from_secs(REQUEST_SECS + 5));
    }
}
