//! A canvas: what the agent shows a person while it works, on a page that
//! changes as they watch it, and what they do there, back to the agent.
//!
//! What is shown is described, not programmed: messages of A2UI (version
//! 0.8, the Agent-to-User Interface protocol), which name components of a
//! catalog (text, pictures, rows, columns, lists, cards, tabs, dividers,
//! buttons, check boxes, text fields, sliders and choices), each by an id,
//! laid out as a list of who holds whom, their values literal or bound to
//! a data model the agent fills (`surfaceUpdate`, `dataModelUpdate`,
//! `beginRendering`, `deleteSurface`). A canvas holds a few surfaces; the
//! page renders them with the browser's own elements, the text as text,
//! and runs nothing the agent sends. A button's action comes back as
//! A2UI's `userAction`, with what the person entered, for the agent to
//! wait for ([`Canvas::wait`]).
//!
//! The page is served at an address of this machine's, known only by the
//! token in its path, as a person's hand-over is
//! ([`crate::takeover`]): a request without the token, or for another host
//! than the address itself, is refused; requests are bounded in size and in
//! time, a few are served at once, and a few people may watch. The page
//! reaches nothing but its own address (its content security policy): a
//! picture is shown only from the bytes the agent gave (`data:` URLs), so
//! what the canvas shows cannot carry anything to another site.
//!
//! The tools are `canvas_show`, which applies messages and shows them to
//! whoever watches, and `canvas_wait`, which waits for what the person does.

use std::collections::{BTreeMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use crate::model::ToolSpec;
use crate::takeover::Timed;
use crate::tools::{Effect, Outcome, Tool};

/// The components a canvas shows, of A2UI's standard catalog.
pub const CATALOG: [&str; 13] = [
    "Text",
    "Image",
    "Row",
    "Column",
    "List",
    "Card",
    "Tabs",
    "Divider",
    "Button",
    "CheckBox",
    "TextField",
    "Slider",
    "MultipleChoice",
];

/// The surface a message names when it names none.
pub const MAIN: &str = "main";

/// The most surfaces a canvas holds.
pub const MAX_SURFACES: usize = 8;

/// The most components a surface holds.
pub const MAX_COMPONENTS: usize = 500;

/// The most bytes of all a canvas holds, as JSON.
pub const MAX_BOARD: usize = 2 << 20;

/// The longest string a component or the data model holds, in characters.
pub const MAX_STRING: usize = 20_000;

/// The longest id, of a surface, a component or an action.
pub const MAX_ID: usize = 64;

/// The most entries of a data model update, nested ones counted.
pub const MAX_ENTRIES: usize = 2_000;

/// The most of the person's actions kept until the agent waits for them.
pub const MAX_ACTIONS: usize = 64;

/// The most people watching at once.
pub const MAX_VIEWERS: usize = 4;

/// How long `canvas_wait` waits unless it says, and at most, in seconds.
pub const WAIT_SECS: u64 = 120;
/// See [`WAIT_SECS`].
pub const MAX_WAIT_SECS: u64 = 600;

/// The most bytes of a request's line and headers.
const MAX_HEAD: usize = 8_192;

/// The most bytes of what the page sends: an action.
const MAX_BODY: usize = 16 << 10;

/// The most headers a request may have.
const MAX_HEADERS: usize = 32;

/// The most requests served at once, watchers among them; more are turned
/// away.
const MAX_SERVING: usize = 12;

/// How long a request may take to arrive, whole.
const REQUEST_SECS: u64 = 10;

/// How often a watcher is sent something, if only to see it is there.
const PING: Duration = Duration::from_secs(15);

/// One surface: its components by id, its data model, and the component
/// it is rendered from, once it is.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Surface {
    /// Its components, each `{ "<Type>": { properties } }`.
    pub components: BTreeMap<String, Value>,
    /// Its data model.
    pub data: Map<String, Value>,
    /// The component it is rendered from.
    pub root: Option<String>,
}

/// What a canvas shows: its surfaces.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Board {
    /// The surfaces, by id.
    pub surfaces: BTreeMap<String, Surface>,
}

fn id_of(value: Option<&Value>, what: &str) -> Result<String, String> {
    let id = value
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{what} needs an id"))?;
    if id.is_empty()
        || id.chars().count() > MAX_ID
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.:".contains(c))
    {
        return Err(format!(
            "{id:?} is not an id: up to {MAX_ID} letters, digits, -, _, . and :"
        ));
    }
    Ok(id.to_string())
}

/// The surface `body` names, [`MAIN`] if none.
fn surface_of(body: &Value) -> Result<String, String> {
    match body.get("surfaceId") {
        None | Some(Value::Null) => Ok(MAIN.to_string()),
        id => id_of(id, "a surface"),
    }
}

/// Checks the strings and the depth of `value`.
fn check_strings(value: &Value, depth: usize) -> Result<(), String> {
    if depth > 16 {
        return Err("a component nests too deep".into());
    }
    match value {
        Value::String(text) if text.chars().count() > MAX_STRING => {
            Err(format!("a string is longer than {MAX_STRING} characters"))
        }
        Value::Array(items) => items.iter().try_for_each(|v| check_strings(v, depth + 1)),
        Value::Object(fields) => fields
            .values()
            .try_for_each(|v| check_strings(v, depth + 1)),
        _ => Ok(()),
    }
}

/// Whether `url` is a picture the page may show: its own bytes.
pub fn picture_url(url: &str) -> bool {
    ["png", "jpeg", "gif", "webp"]
        .iter()
        .any(|kind| url.starts_with(&format!("data:image/{kind};base64,")))
}

/// Checks one component, `{ "<Type>": { properties } }`.
fn check_component(id: &str, component: &Value) -> Result<(), String> {
    let fields = component
        .as_object()
        .filter(|f| f.len() == 1)
        .ok_or_else(|| format!("component {id} is one type and its properties"))?;
    let (kind, properties) = fields.iter().next().expect("one field");
    if !CATALOG.contains(&kind.as_str()) {
        return Err(format!(
            "{kind} (component {id}) is not a component the canvas shows: {}",
            CATALOG.join(", ")
        ));
    }
    if !properties.is_object() {
        return Err(format!("component {id}'s properties are an object"));
    }
    check_strings(properties, 0)?;
    match kind.as_str() {
        "Image" => {
            if let Some(url) = properties["url"]["literalString"].as_str() {
                if !picture_url(url) {
                    return Err(format!(
                        "component {id}'s picture is not shown from another site: give its \
                         bytes, a data:image/png (or jpeg, gif, webp);base64 URL"
                    ));
                }
            }
        }
        "Button" => {
            let name = properties["action"]["name"].as_str().unwrap_or_default();
            if name.is_empty() || name.chars().count() > MAX_ID {
                return Err(format!(
                    "button {id} needs an action with a name of up to {MAX_ID} characters"
                ));
            }
        }
        _ => {}
    }
    Ok(())
}

/// A data model update's entries, as JSON, counting them in `count`.
fn entries(list: &Value, count: &mut usize, depth: usize) -> Result<Map<String, Value>, String> {
    if depth > 8 {
        return Err("the data model nests too deep".into());
    }
    let list = list
        .as_array()
        .ok_or("a data model update's contents are a list of entries")?;
    let mut out = Map::new();
    for entry in list {
        *count += 1;
        if *count > MAX_ENTRIES {
            return Err(format!(
                "a data model update has at most {MAX_ENTRIES} entries"
            ));
        }
        let key = entry["key"]
            .as_str()
            .filter(|k| !k.is_empty() && k.chars().count() <= 256 && !k.contains('/'))
            .ok_or("each entry has a key: up to 256 characters, without /")?;
        let value = if let Some(text) = entry.get("valueString") {
            let text = text.as_str().ok_or("valueString is a string")?;
            if text.chars().count() > MAX_STRING {
                return Err(format!("a value is longer than {MAX_STRING} characters"));
            }
            json!(text)
        } else if let Some(number) = entry.get("valueNumber") {
            json!(number.as_f64().ok_or("valueNumber is a number")?)
        } else if let Some(flag) = entry.get("valueBoolean") {
            json!(flag.as_bool().ok_or("valueBoolean is true or false")?)
        } else if let Some(map) = entry.get("valueMap") {
            Value::Object(entries(map, count, depth + 1)?)
        } else {
            return Err(format!(
                "entry {key} has no value: valueString, valueNumber, valueBoolean or valueMap"
            ));
        };
        out.insert(key.to_string(), value);
    }
    Ok(out)
}

/// The parts of a data model's path: `/a/b` is `a`, `b`.
fn path_parts(path: &str) -> Result<Vec<String>, String> {
    let parts: Vec<String> = path
        .split('/')
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect();
    if parts.len() > 8 || parts.iter().any(|p| p.chars().count() > 256) {
        return Err(format!("{path:?} is not a path of the data model"));
    }
    Ok(parts)
}

/// A data model as a data model update's entries.
fn as_entries(data: &Map<String, Value>) -> Vec<Value> {
    data.iter()
        .map(|(key, value)| match value {
            Value::String(text) => json!({"key": key, "valueString": text}),
            Value::Number(n) => json!({"key": key, "valueNumber": n}),
            Value::Bool(b) => json!({"key": key, "valueBoolean": b}),
            Value::Object(map) => json!({"key": key, "valueMap": as_entries(map)}),
            other => json!({"key": key, "valueString": other.to_string()}),
        })
        .collect()
}

impl Board {
    /// Applies `messages`, all or none: the messages as they are shown, each
    /// naming its surface.
    pub fn apply(&mut self, messages: &[Value]) -> Result<Vec<Value>, String> {
        let mut next = self.clone();
        let mut shown = Vec::new();
        for message in messages {
            shown.push(next.apply_one(message)?);
        }
        let size = serde_json::to_vec(&next.snapshot())
            .map(|b| b.len())
            .unwrap_or(usize::MAX);
        if size > MAX_BOARD {
            return Err(format!(
                "the canvas would hold {size} bytes, more than {MAX_BOARD}: take something off"
            ));
        }
        *self = next;
        Ok(shown)
    }

    fn apply_one(&mut self, message: &Value) -> Result<Value, String> {
        let fields = message
            .as_object()
            .filter(|f| f.len() == 1)
            .ok_or("each message is one of surfaceUpdate, dataModelUpdate, beginRendering or deleteSurface")?;
        let (kind, body) = fields.iter().next().expect("one field");
        let surface_id = surface_of(body)?;
        if kind != "deleteSurface"
            && !self.surfaces.contains_key(&surface_id)
            && self.surfaces.len() >= MAX_SURFACES
        {
            return Err(format!("a canvas holds at most {MAX_SURFACES} surfaces"));
        }
        match kind.as_str() {
            "surfaceUpdate" => {
                let components = body["components"]
                    .as_array()
                    .ok_or("surfaceUpdate gives components: a list of {id, component}")?;
                let surface = self.surfaces.entry(surface_id.clone()).or_default();
                for component in components {
                    let id = id_of(component.get("id"), "a component")?;
                    check_component(&id, &component["component"])?;
                    surface
                        .components
                        .insert(id, component["component"].clone());
                }
                if surface.components.len() > MAX_COMPONENTS {
                    return Err(format!(
                        "a surface holds at most {MAX_COMPONENTS} components"
                    ));
                }
                Ok(json!({"surfaceUpdate": {"surfaceId": surface_id, "components": components}}))
            }
            "dataModelUpdate" => {
                let path = body["path"].as_str().unwrap_or("/");
                let parts = path_parts(path)?;
                let mut count = 0;
                let added = entries(&body["contents"], &mut count, 0)?;
                let surface = self.surfaces.entry(surface_id.clone()).or_default();
                let mut at = &mut surface.data;
                for part in &parts {
                    let next = at
                        .entry(part.clone())
                        .or_insert_with(|| Value::Object(Map::new()));
                    if !next.is_object() {
                        *next = Value::Object(Map::new());
                    }
                    at = next.as_object_mut().expect("an object");
                }
                at.extend(added);
                let mut shown = json!({"surfaceId": surface_id, "contents": body["contents"]});
                if !parts.is_empty() {
                    shown["path"] = json!(format!("/{}", parts.join("/")));
                }
                Ok(json!({ "dataModelUpdate": shown }))
            }
            "beginRendering" => {
                let root = id_of(body.get("root"), "beginRendering's root")?;
                let surface = self.surfaces.entry(surface_id.clone()).or_default();
                if !surface.components.contains_key(&root) {
                    return Err(format!(
                        "surface {surface_id} has no component {root} to render from: send it \
                         in a surfaceUpdate first"
                    ));
                }
                surface.root = Some(root.clone());
                Ok(json!({"beginRendering": {"surfaceId": surface_id, "root": root}}))
            }
            "deleteSurface" => {
                self.surfaces.remove(&surface_id);
                Ok(json!({"deleteSurface": {"surfaceId": surface_id}}))
            }
            other => Err(format!(
                "{other} is not an A2UI message: surfaceUpdate, dataModelUpdate, beginRendering \
                 or deleteSurface"
            )),
        }
    }

    /// The messages that show the canvas as it is, to someone who starts
    /// watching.
    pub fn snapshot(&self) -> Vec<Value> {
        let mut out = Vec::new();
        for (id, surface) in &self.surfaces {
            let components: Vec<Value> = surface
                .components
                .iter()
                .map(|(cid, component)| json!({"id": cid, "component": component}))
                .collect();
            out.push(json!({"surfaceUpdate": {"surfaceId": id, "components": components}}));
            if !surface.data.is_empty() {
                out.push(json!({"dataModelUpdate": {
                    "surfaceId": id,
                    "contents": as_entries(&surface.data),
                }}));
            }
            if let Some(root) = &surface.root {
                out.push(json!({"beginRendering": {"surfaceId": id, "root": root}}));
            }
        }
        out
    }

    /// Checks a person's action, `{"userAction": {...}}`: of a button on
    /// the canvas, with that button's action's name. Returns the action.
    pub fn check_action(&self, sent: &Value) -> Result<Value, String> {
        let action = sent
            .get("userAction")
            .filter(|a| a.is_object())
            .ok_or("no userAction")?;
        let surface_id = surface_of(action)?;
        let surface = self.surfaces.get(&surface_id).ok_or("no such surface")?;
        let source = id_of(action.get("sourceComponentId"), "the action's source")?;
        let button = surface
            .components
            .get(&source)
            .and_then(|c| c.get("Button"))
            .ok_or("no such button")?;
        let name = action["name"].as_str().unwrap_or_default();
        if button["action"]["name"].as_str() != Some(name) {
            return Err("not that button's action".into());
        }
        let context = match action.get("context") {
            None | Some(Value::Null) => json!({}),
            Some(context) if context.is_object() => context.clone(),
            Some(_) => return Err("the action's context is an object".into()),
        };
        check_strings(&context, 0)?;
        Ok(json!({
            "name": name,
            "surfaceId": surface_id,
            "sourceComponentId": source,
            "timestamp": action["timestamp"].as_str().unwrap_or_default(),
            "context": context,
        }))
    }
}

/// The messages as one event of Server-Sent Events, named `kind` if not
/// the default.
fn event(kind: Option<&str>, messages: &[Value]) -> String {
    let mut out = String::new();
    if let Some(kind) = kind {
        out.push_str(&format!("event: {kind}\n"));
    }
    for message in messages {
        out.push_str(&format!("data: {message}\n"));
    }
    if messages.is_empty() {
        out.push_str("data: \n");
    }
    out.push('\n');
    out
}

struct Inner {
    board: Mutex<Board>,
    watchers: Mutex<Vec<mpsc::Sender<String>>>,
    watching: AtomicUsize,
    actions: Mutex<VecDeque<Value>>,
    arrived: Condvar,
    url: OnceLock<String>,
    nonce: String,
}

/// A canvas, started when first shown.
#[derive(Clone)]
pub struct Canvas {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Canvas {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Canvas")
            .field("url", &self.inner.url.get())
            .finish_non_exhaustive()
    }
}

impl Default for Canvas {
    fn default() -> Canvas {
        Canvas::new()
    }
}

/// A request being served, counted while it is.
struct Serving(Arc<AtomicUsize>);

impl Drop for Serving {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

fn hex_random(bytes: usize) -> Result<String, String> {
    let mut buffer = vec![0u8; bytes];
    sion_http::random(&mut buffer).map_err(|e| e.to_string())?;
    Ok(crate::encoding::hex(&buffer))
}

impl Canvas {
    /// A canvas with nothing on it, not yet served.
    pub fn new() -> Canvas {
        Canvas {
            inner: Arc::new(Inner {
                board: Mutex::new(Board::default()),
                watchers: Mutex::new(Vec::new()),
                watching: AtomicUsize::new(0),
                actions: Mutex::new(VecDeque::new()),
                arrived: Condvar::new(),
                url: OnceLock::new(),
                nonce: hex_random(16).unwrap_or_else(|_| "0".repeat(32)),
            }),
        }
    }

    /// Where a person sees it, `http://127.0.0.1:<port>/<token>`: served
    /// from the first time it is asked.
    pub fn url(&self) -> Result<String, String> {
        if let Some(url) = self.inner.url.get() {
            return Ok(url.clone());
        }
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let token = hex_random(16)?;
        let url = format!("http://127.0.0.1:{port}/{token}");
        if self.inner.url.set(url.clone()).is_err() {
            // Another call served it first.
            return Ok(self.inner.url.get().cloned().unwrap_or(url));
        }
        let weak = Arc::downgrade(&self.inner);
        let serving = Arc::new(AtomicUsize::new(0));
        std::thread::Builder::new()
            .name("sion-canvas".into())
            .spawn(move || accept(listener, weak, &token, port, &serving))
            .map_err(|e| e.to_string())?;
        Ok(url)
    }

    /// Applies `messages` and shows them to whoever watches: what it shows
    /// now, in a line.
    pub fn show(&self, messages: &[Value]) -> Result<String, String> {
        if messages.is_empty() {
            return Err("canvas_show needs messages".into());
        }
        let url = self.url()?;
        let (shown, summary) = {
            let mut board = self.inner.board.lock().unwrap_or_else(|p| p.into_inner());
            let shown = board.apply(messages)?;
            let summary: Vec<String> = board
                .surfaces
                .iter()
                .map(|(id, s)| {
                    format!(
                        "{id}: {} components{}",
                        s.components.len(),
                        match &s.root {
                            Some(root) => format!(", shown from {root}"),
                            None => ", not shown yet (beginRendering)".into(),
                        }
                    )
                })
                .collect();
            (shown, summary)
        };
        let text = event(None, &shown);
        self.inner
            .watchers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .retain(|watcher| watcher.send(text.clone()).is_ok());
        let watching = self.inner.watching.load(Ordering::SeqCst);
        Ok(format!(
            "the canvas holds {}; {} watching it at {url}",
            if summary.is_empty() {
                "nothing".to_string()
            } else {
                summary.join("; ")
            },
            match watching {
                0 => "no one is".to_string(),
                1 => "1 person is".to_string(),
                n => format!("{n} people are"),
            }
        ))
    }

    /// What the canvas holds now.
    pub fn board(&self) -> Board {
        self.inner
            .board
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// The person's actions since the last wait, waiting at most `wait`
    /// for the first: each A2UI's `userAction`, checked.
    pub fn wait(&self, wait: Duration) -> Vec<Value> {
        let deadline = Instant::now() + wait;
        let mut actions = self.inner.actions.lock().unwrap_or_else(|p| p.into_inner());
        while actions.is_empty() {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            actions = self
                .inner
                .arrived
                .wait_timeout(actions, left)
                .unwrap_or_else(|p| p.into_inner())
                .0;
        }
        actions.drain(..).collect()
    }

    /// Its tools: `canvas_show` and `canvas_wait`.
    pub fn tools(&self) -> Vec<Box<dyn Tool>> {
        vec![Box::new(Show(self.clone())), Box::new(Wait(self.clone()))]
    }
}

fn accept(
    listener: TcpListener,
    canvas: Weak<Inner>,
    token: &str,
    port: u16,
    serving: &Arc<AtomicUsize>,
) {
    loop {
        if canvas.strong_count() == 0 {
            return;
        }
        match listener.accept() {
            Ok((stream, _)) => {
                if serving.fetch_add(1, Ordering::SeqCst) >= MAX_SERVING {
                    serving.fetch_sub(1, Ordering::SeqCst);
                    continue;
                }
                let counted = Serving(serving.clone());
                let (canvas, token) = (canvas.clone(), token.to_string());
                // A thread that cannot be made drops the connection, and
                // its count with it.
                let _ = std::thread::Builder::new().spawn(move || {
                    let _counted = counted;
                    let _ = serve(stream, &canvas, &token, port);
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(30));
            }
            Err(_) => std::thread::sleep(Duration::from_millis(200)),
        }
    }
}

fn respond(
    stream: &mut TcpStream,
    status: &str,
    kind: &str,
    body: &[u8],
    nonce: &str,
) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\n\
         X-Content-Type-Options: nosniff\r\nX-Frame-Options: DENY\r\nReferrer-Policy: no-referrer\r\n\
         {}Connection: close\r\n\r\n",
        body.len(),
        policy(nonce)
    )?;
    stream.write_all(body)?;
    stream.flush()
}

/// The page's content security policy: its own script and style, pictures
/// from their bytes, and nothing reached but its own address.
fn policy(nonce: &str) -> String {
    format!(
        "Content-Security-Policy: default-src 'none'; script-src 'nonce-{nonce}'; style-src \
         'nonce-{nonce}'; img-src data:; connect-src 'self'; base-uri 'none'; form-action 'none'; \
         frame-ancestors 'none'\r\n"
    )
}

/// A watcher, counted while it watches.
struct Watching(Arc<Inner>);

impl Drop for Watching {
    fn drop(&mut self) {
        self.0.watching.fetch_sub(1, Ordering::SeqCst);
    }
}

fn serve(
    mut stream: TcpStream,
    canvas: &Weak<Inner>,
    token: &str,
    port: u16,
) -> std::io::Result<()> {
    stream.set_nonblocking(false)?;
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
    let Some(inner) = canvas.upgrade() else {
        return Ok(());
    };
    let nonce = inner.nonce.clone();
    if !ended || !line.ends_with('\n') {
        return respond(
            &mut stream,
            "431 Request Header Fields Too Large",
            "text/plain",
            b"too long",
            &nonce,
        );
    }
    // Only this address, by its own name.
    if host != format!("127.0.0.1:{port}") && host != format!("localhost:{port}") {
        return respond(
            &mut stream,
            "403 Forbidden",
            "text/plain",
            b"not here",
            &nonce,
        );
    }
    if length > MAX_BODY {
        return respond(
            &mut stream,
            "413 Payload Too Large",
            "text/plain",
            b"too long",
            &nonce,
        );
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body)?;
    let path = path.split('?').next().unwrap_or_default();
    let Some(rest) = path.strip_prefix('/').and_then(|p| p.strip_prefix(token)) else {
        return respond(&mut stream, "404 Not Found", "text/plain", b"", &nonce);
    };
    match (method.as_str(), rest) {
        ("GET", "" | "/") => respond(
            &mut stream,
            "200 OK",
            "text/html; charset=utf-8",
            PAGE.replace("{NONCE}", &nonce).as_bytes(),
            &nonce,
        ),
        ("GET", "/events") => {
            if inner.watching.fetch_add(1, Ordering::SeqCst) >= MAX_VIEWERS {
                inner.watching.fetch_sub(1, Ordering::SeqCst);
                return respond(
                    &mut stream,
                    "429 Too Many Requests",
                    "text/plain",
                    b"enough watchers",
                    &nonce,
                );
            }
            let watching = Watching(inner.clone());
            let (sender, receiver) = mpsc::channel();
            let snapshot = {
                let board = inner.board.lock().unwrap_or_else(|p| p.into_inner());
                // Taken with the board held, so that nothing shown after
                // the snapshot is missed nor sent twice.
                inner
                    .watchers
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .push(sender);
                event(Some("snapshot"), &board.snapshot())
            };
            drop(inner);
            stream.set_write_timeout(Some(Duration::from_secs(REQUEST_SECS)))?;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-store\r\n\
                 X-Content-Type-Options: nosniff\r\nConnection: keep-alive\r\n\r\n"
            )?;
            stream.write_all(snapshot.as_bytes())?;
            stream.flush()?;
            let result = loop {
                match receiver.recv_timeout(PING) {
                    Ok(text) => {
                        if let Err(e) = stream
                            .write_all(text.as_bytes())
                            .and_then(|()| stream.flush())
                        {
                            break Err(e);
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if let Err(e) = stream
                            .write_all(b": ping\n\n")
                            .and_then(|()| stream.flush())
                        {
                            break Err(e);
                        }
                    }
                    // The canvas is gone.
                    Err(mpsc::RecvTimeoutError::Disconnected) => break Ok(()),
                }
            };
            drop(watching);
            result
        }
        ("POST", "/action") => {
            let sent: Value = match serde_json::from_slice(&body) {
                Ok(sent) => sent,
                Err(_) => {
                    return respond(
                        &mut stream,
                        "400 Bad Request",
                        "text/plain",
                        b"not JSON",
                        &nonce,
                    )
                }
            };
            let checked = inner
                .board
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .check_action(&sent);
            match checked {
                Ok(action) => {
                    let mut actions = inner.actions.lock().unwrap_or_else(|p| p.into_inner());
                    if actions.len() >= MAX_ACTIONS {
                        actions.pop_front();
                    }
                    actions.push_back(action);
                    inner.arrived.notify_all();
                    drop(actions);
                    respond(&mut stream, "204 No Content", "text/plain", b"", &nonce)
                }
                Err(why) => respond(
                    &mut stream,
                    "400 Bad Request",
                    "text/plain",
                    why.as_bytes(),
                    &nonce,
                ),
            }
        }
        _ => respond(&mut stream, "404 Not Found", "text/plain", b"", &nonce),
    }
}

/// `canvas_show`.
struct Show(Canvas);

/// `canvas_wait`.
struct Wait(Canvas);

impl Tool for Show {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "canvas_show".into(),
            description: format!(
                "Show the person something on your canvas, a page they watch that changes as you \
                 send it: a summary, a table of rows, choices, a form. Give messages of A2UI \
                 (v0.8): surfaceUpdate {{surfaceId, components: [{{id, component: {{Type: \
                 {{...}}}}}}]}}, dataModelUpdate {{surfaceId, path, contents: [{{key, \
                 valueString | valueNumber | valueBoolean | valueMap}}]}}, beginRendering \
                 {{surfaceId, root}}, deleteSurface {{surfaceId}}. Components: {}. Text {{text, \
                 usageHint: h1-h5, caption, body}}; Image {{url: a data:image URL}}; Row, Column, \
                 List {{children: {{explicitList: [ids]}} or {{template: {{dataBinding: path, \
                 componentId}}}}}}; Card {{child}}; Tabs {{tabItems: [{{title, child}}]}}; Button \
                 {{child, primary, action: {{name, context: [{{key, value}}]}}}}; CheckBox {{label, \
                 value}}; TextField {{label, text, textFieldType: shortText, longText, number, \
                 date, obscured}}; Slider {{value, minValue, maxValue}}; MultipleChoice \
                 {{selections, options: [{{label, value}}], maxAllowedSelections}}. A value is \
                 {{literalString: ...}} (or literalNumber, literalBoolean) or {{path: \
                 \"/total\"}}, bound to the data model (a path without / is the template's item). \
                 Send components, then beginRendering once; later, send what changes. A button \
                 sends you its action with its context: wait for it with canvas_wait.",
                CATALOG.join(", ")
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "messages": {
                        "type": "array",
                        "items": {"type": "object"},
                        "description": "A2UI messages, applied in order, all or none"
                    }
                },
                "required": ["messages"]
            }),
        }
    }

    /// What it shows reaches only the person who watches, on this machine.
    fn effect(&self) -> Effect {
        Effect::Write
    }

    fn journaled(&self) -> bool {
        false
    }

    fn call(&self, input: &Value, _key: &str) -> Outcome {
        let Some(messages) = input["messages"].as_array() else {
            return Outcome::error("canvas_show needs messages: a list of A2UI messages");
        };
        match self.0.show(messages) {
            Ok(text) => Outcome::ok(text),
            Err(why) => Outcome::error(why),
        }
    }
}

/// What the person did, as the model is told it.
pub fn actions_report(actions: &[Value]) -> String {
    if actions.is_empty() {
        return "the person has done nothing on the canvas yet".into();
    }
    actions
        .iter()
        .map(|a| {
            format!(
                "the person pressed {} ({} on {}){}",
                a["name"].as_str().unwrap_or_default(),
                a["sourceComponentId"].as_str().unwrap_or_default(),
                a["surfaceId"].as_str().unwrap_or_default(),
                match a["context"].as_object() {
                    Some(context) if !context.is_empty() => format!(", with {}", a["context"]),
                    _ => String::new(),
                }
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

impl Tool for Wait {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "canvas_wait".into(),
            description: format!(
                "Wait for the person to act on your canvas (a button, with what they entered): \
                 what they did since you last asked, waiting up to seconds (by default \
                 {WAIT_SECS}, at most {MAX_WAIT_SECS}) for the first."
            ),
            input_schema: json!({
                "type": "object",
                "properties": {"seconds": {"type": "integer"}}
            }),
        }
    }

    fn effect(&self) -> Effect {
        Effect::Read
    }

    fn journaled(&self) -> bool {
        false
    }

    fn waits_for_person(&self) -> bool {
        true
    }

    fn call(&self, input: &Value, _key: &str) -> Outcome {
        let secs = input["seconds"]
            .as_u64()
            .unwrap_or(WAIT_SECS)
            .clamp(1, MAX_WAIT_SECS);
        Outcome::ok(actions_report(&self.0.wait(Duration::from_secs(secs))))
    }
}

/// The page: the canvas's surfaces, rendered from what the agent sent, the
/// person's actions sent back.
const PAGE: &str = r##"<!doctype html><html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>The agent's canvas</title>
<style nonce="{NONCE}">
:root{--bg:#f6f5f2;--card:#fff;--fg:#1d1c1a;--muted:#6b6862;--line:#dedad3;--accent:#2f5d8a;--accent-fg:#fff;color-scheme:light}
@media (prefers-color-scheme: dark){:root{--bg:#161615;--card:#201f1d;--fg:#ecebe7;--muted:#a19d95;--line:#37352f;--accent:#7fb0e0;--accent-fg:#10202f;color-scheme:dark}}
*{box-sizing:border-box}
body{margin:0;background:var(--bg);color:var(--fg);font:15px/1.5 system-ui,-apple-system,"Segoe UI",sans-serif}
header{display:flex;justify-content:space-between;align-items:baseline;gap:12px;padding:12px max(16px,env(safe-area-inset-left));border-bottom:1px solid var(--line)}
header span{color:var(--muted);font-size:13px}
main{max-width:980px;margin:0 auto;padding:20px max(16px,env(safe-area-inset-left));display:flex;flex-direction:column;gap:24px}
#empty{color:var(--muted)}
.surface{display:flex;flex-direction:column;gap:12px;min-width:0}
.surface:empty{display:none}
.row,.column,.list{display:flex;gap:12px;min-width:0}
.row,.list.across{flex-direction:row;flex-wrap:wrap}
.column,.list{flex-direction:column}
[data-align=center]{align-items:center}[data-align=end]{align-items:flex-end}[data-align=start]{align-items:flex-start}[data-align=stretch]{align-items:stretch}
[data-distribution=spaceBetween]{justify-content:space-between}[data-distribution=center]{justify-content:center}[data-distribution=end]{justify-content:flex-end}[data-distribution=spaceAround]{justify-content:space-around}
.row>*{flex:0 1 auto;min-width:0}
.card{background:var(--card);border:1px solid var(--line);border-radius:10px;padding:14px 16px;min-width:0}
.text{margin:0;overflow-wrap:anywhere;white-space:pre-wrap}
h1.text{font-size:28px;line-height:1.2}h2.text{font-size:22px;line-height:1.25}h3.text{font-size:18px}h4.text,h5.text{font-size:16px}
.text.caption{color:var(--muted);font-size:13px}
.image{max-width:100%;height:auto;border-radius:6px}
.divider{border:0;border-top:1px solid var(--line);margin:4px 0;width:100%}
.divider.vertical{border-top:0;border-left:1px solid var(--line);width:auto;align-self:stretch}
.button{font:inherit;padding:8px 14px;border-radius:8px;border:1px solid var(--line);background:var(--card);color:var(--fg);cursor:pointer;align-self:flex-start}
.button.primary{background:var(--accent);border-color:var(--accent);color:var(--accent-fg)}
.button .text{white-space:normal}
.button:focus-visible,.tab:focus-visible,input:focus-visible,textarea:focus-visible{outline:2px solid var(--accent);outline-offset:2px}
.field{display:flex;flex-direction:column;gap:4px}
.field span,.slider span{font-size:13px;color:var(--muted)}
.field input,.field textarea{font:inherit;padding:8px 10px;border-radius:8px;border:1px solid var(--line);background:var(--card);color:var(--fg);width:100%}
.field textarea{min-height:90px}
.checkbox,.choice label{display:flex;gap:8px;align-items:center}
.choice{border:1px solid var(--line);border-radius:8px;padding:8px 12px;display:flex;flex-direction:column;gap:6px}
.slider{display:flex;gap:10px;align-items:center}
.tabs{display:flex;flex-direction:column;gap:12px}
.tabbar{display:flex;gap:4px;border-bottom:1px solid var(--line);flex-wrap:wrap}
.tab{font:inherit;padding:6px 12px;border:0;border-bottom:2px solid transparent;background:none;color:var(--muted);cursor:pointer}
.tab.chosen{color:var(--fg);border-bottom-color:var(--accent)}
.missing{color:var(--muted);font-size:13px}
</style></head><body>
<header><strong>The agent's canvas</strong><span id="state">Connecting…</span></header>
<main id="surfaces"><p id="empty">Nothing on the canvas yet: the agent shows things here as it works.</p></main>
<script nonce="{NONCE}">
(() => {
  'use strict';
  const base = location.pathname.replace(/\/+$/, '');
  const holder = document.getElementById('surfaces');
  const state = document.getElementById('state');
  const empty = document.getElementById('empty');
  const surfaces = new Map();
  const LITERALS = ['literalString', 'literalNumber', 'literalBoolean', 'literalArray'];
  const say = (text) => { state.textContent = text; };
  const make = (tag, cls) => { const e = document.createElement(tag); if (cls) e.className = cls; return e; };
  const parts = (path) => String(path || '').split('/').filter((p) => p.length > 0);
  const absolute = (path, scope) => (String(path).startsWith('/') ? String(path) : scope + '/' + path);
  const at = (data, path) => { let node = data; for (const p of parts(path)) { if (node === null || typeof node !== 'object') return undefined; node = node[p]; } return node; };
  const put = (data, path, v) => {
    const ps = parts(path); if (!ps.length) return; let node = data;
    for (const p of ps.slice(0, -1)) { if (node[p] === null || typeof node[p] !== 'object') node[p] = {}; node = node[p]; }
    node[ps[ps.length - 1]] = v;
  };
  const shown = () => { empty.hidden = Array.from(surfaces.values()).some((s) => s.root); };

  function surface(id) {
    let s = surfaces.get(id);
    if (!s) { s = { id, components: new Map(), data: {}, root: null, tabs: {}, el: make('section', 'surface') }; surfaces.set(id, s); holder.appendChild(s.el); }
    return s;
  }
  function bound(b, s, scope) {
    if (b === null || b === undefined) return undefined;
    if (typeof b !== 'object' || Array.isArray(b)) return b;
    if (typeof b.path === 'string') {
      const p = absolute(b.path, scope); const found = at(s.data, p);
      if (found !== undefined) return found;
      for (const k of LITERALS) if (k in b) { put(s.data, p, b[k]); return b[k]; }
      return undefined;
    }
    for (const k of LITERALS) if (k in b) return b[k];
    return undefined;
  }
  const text = (b, s, scope) => { const v = bound(b, s, scope); return v === undefined || v === null ? '' : (typeof v === 'object' ? JSON.stringify(v) : String(v)); };
  function entries(list) {
    const out = {};
    for (const e of Array.isArray(list) ? list : []) {
      if (!e || typeof e.key !== 'string') continue;
      if ('valueString' in e) out[e.key] = e.valueString; else if ('valueNumber' in e) out[e.key] = e.valueNumber;
      else if ('valueBoolean' in e) out[e.key] = e.valueBoolean; else if ('valueMap' in e) out[e.key] = entries(e.valueMap);
    }
    return out;
  }
  function apply(m) {
    if (m.surfaceUpdate) { const s = surface(m.surfaceUpdate.surfaceId || 'main'); for (const c of m.surfaceUpdate.components || []) s.components.set(c.id, c.component); return s.id; }
    if (m.dataModelUpdate) {
      const u = m.dataModelUpdate; const s = surface(u.surfaceId || 'main'); const add = entries(u.contents);
      if (!parts(u.path).length) Object.assign(s.data, add);
      else { let t = at(s.data, u.path); if (t === null || typeof t !== 'object') { put(s.data, u.path, {}); t = at(s.data, u.path); } Object.assign(t, add); }
      return s.id;
    }
    if (m.beginRendering) { const s = surface(m.beginRendering.surfaceId || 'main'); s.root = m.beginRendering.root; return s.id; }
    if (m.deleteSurface) { const id = m.deleteSurface.surfaceId || 'main'; const s = surfaces.get(id); if (s) { s.el.remove(); surfaces.delete(id); } return null; }
    return null;
  }
  const missing = (what) => { const e = make('span', 'missing'); e.textContent = what; return e; };
  function children(s, list, scope, path, depth) {
    if (!list || typeof list !== 'object') return [];
    if (Array.isArray(list.explicitList)) return list.explicitList.map((c) => build(s, c, scope, path, depth + 1));
    if (list.template && typeof list.template === 'object') {
      const t = list.template; const where = absolute(t.dataBinding || '', scope); const items = at(s.data, where);
      if (!items || typeof items !== 'object') return [];
      return Object.keys(items).slice(0, 500).map((k) => build(s, t.componentId, where + '/' + k, path, depth + 1));
    }
    return [];
  }
  function build(s, id, scope, path, depth) {
    const c = s.components.get(id);
    if (!c || typeof c !== 'object') return missing('[' + id + ']');
    if (depth > 48 || path.includes(id)) return missing('[' + id + ': a loop]');
    const type = Object.keys(c)[0];
    const p = c[type] && typeof c[type] === 'object' ? c[type] : {};
    const e = component(type, p, s, scope, path.concat([id]), depth, id);
    e.dataset.component = id + (scope ? '@' + scope : '');
    const weight = Number(p.weight);
    if (weight > 0) e.style.flexGrow = String(weight);
    return e;
  }
  function component(type, p, s, scope, path, depth, id) {
    const sub = (child) => build(s, child, scope, path, depth + 1);
    switch (type) {
      case 'Text': {
        const hint = String(p.usageHint || 'body'); const e = make(/^h[1-5]$/.test(hint) ? hint : 'p', 'text ' + hint);
        e.textContent = text(p.text, s, scope); return e;
      }
      case 'Image': {
        const e = make('img', 'image'); const url = text(p.url, s, scope);
        if (/^data:image\/(png|jpeg|gif|webp);base64,/.test(url)) e.src = url;
        e.alt = text(p.altText, s, scope); return e;
      }
      case 'Row': case 'Column': case 'List': {
        const e = make('div', type.toLowerCase());
        if (type === 'List' && p.direction === 'horizontal') e.classList.add('across');
        if (p.alignment) e.dataset.align = String(p.alignment);
        if (p.distribution) e.dataset.distribution = String(p.distribution);
        for (const k of children(s, p.children, scope, path, depth)) e.appendChild(k);
        return e;
      }
      case 'Card': { const e = make('div', 'card'); if (p.child) e.appendChild(sub(p.child)); return e; }
      case 'Divider': return make('hr', p.axis === 'vertical' ? 'divider vertical' : 'divider');
      case 'Tabs': {
        const e = make('div', 'tabs'); const bar = make('div', 'tabbar'); const items = Array.isArray(p.tabItems) ? p.tabItems : [];
        const key = id + '@' + scope; const chosen = Math.min(s.tabs[key] || 0, Math.max(items.length - 1, 0));
        items.forEach((item, i) => {
          const b = make('button', i === chosen ? 'tab chosen' : 'tab'); b.type = 'button';
          b.textContent = text(item && item.title, s, scope); b.onclick = () => { s.tabs[key] = i; render(s); }; bar.appendChild(b);
        });
        e.appendChild(bar); if (items[chosen] && items[chosen].child) e.appendChild(sub(items[chosen].child)); return e;
      }
      case 'Button': {
        const e = make('button', p.primary ? 'button primary' : 'button'); e.type = 'button';
        if (p.child) e.appendChild(sub(p.child)); e.onclick = () => act(s, id, p.action || {}, scope); return e;
      }
      case 'CheckBox': {
        const e = make('label', 'checkbox'); const box = make('input'); box.type = 'checkbox';
        box.checked = Boolean(bound(p.value, s, scope));
        box.onchange = () => { if (p.value && typeof p.value.path === 'string') put(s.data, absolute(p.value.path, scope), box.checked); };
        const t = make('span'); t.textContent = text(p.label, s, scope); e.append(box, t); return e;
      }
      case 'TextField': {
        const e = make('label', 'field'); const t = make('span'); t.textContent = text(p.label, s, scope);
        const long = p.textFieldType === 'longText'; const input = make(long ? 'textarea' : 'input');
        if (!long) input.type = ({ number: 'number', date: 'date', obscured: 'password' })[p.textFieldType] || 'text';
        input.value = text(p.text, s, scope);
        input.oninput = () => { if (p.text && typeof p.text.path === 'string') put(s.data, absolute(p.text.path, scope), input.value); };
        e.append(t, input); return e;
      }
      case 'Slider': {
        const e = make('label', 'slider'); const input = make('input'); input.type = 'range';
        const min = Number(p.minValue ?? 0), max = Number(p.maxValue ?? 100);
        input.min = String(min); input.max = String(max); input.value = String(Number(bound(p.value, s, scope) ?? min));
        const out = make('span'); out.textContent = input.value;
        input.oninput = () => { out.textContent = input.value; if (p.value && typeof p.value.path === 'string') put(s.data, absolute(p.value.path, scope), Number(input.value)); };
        e.append(input, out); return e;
      }
      case 'MultipleChoice': {
        const e = make('fieldset', 'choice'); const most = Number(p.maxAllowedSelections) || Infinity; const one = most === 1;
        const where = p.selections && typeof p.selections.path === 'string' ? absolute(p.selections.path, scope) : null;
        let chosen = bound(p.selections, s, scope); if (!Array.isArray(chosen)) chosen = [];
        const name = 'choice-' + id + '-' + scope;
        for (const option of Array.isArray(p.options) ? p.options : []) {
          const row = make('label'); const input = make('input'); input.type = one ? 'radio' : 'checkbox'; input.name = name;
          const v = String(option && option.value); input.checked = chosen.map(String).includes(v);
          input.onchange = () => {
            let now = one ? [] : chosen.slice();
            if (input.checked) { if (!now.includes(v)) now.push(v); } else now = now.filter((x) => x !== v);
            if (now.length > most) { input.checked = false; return; }
            chosen = now; if (where) put(s.data, where, now);
          };
          const t = make('span'); t.textContent = text(option && option.label, s, scope); row.append(input, t); e.appendChild(row);
        }
        return e;
      }
      default: return missing('[' + type + ']');
    }
  }
  function render(s) {
    const active = document.activeElement;
    const holderOf = active && s.el.contains(active) ? active.closest('[data-component]') : null;
    const key = holderOf ? holderOf.dataset.component : null;
    const selection = active && typeof active.selectionStart === 'number' ? [active.selectionStart, active.selectionEnd] : null;
    s.el.replaceChildren();
    if (s.root) s.el.appendChild(build(s, s.root, '', [], 0));
    if (key) {
      const again = Array.from(s.el.querySelectorAll('[data-component]')).find((x) => x.dataset.component === key);
      const target = again && (again.matches('input,textarea,button') ? again : again.querySelector('input,textarea,button'));
      if (target) { target.focus(); if (selection && typeof target.setSelectionRange === 'function') { try { target.setSelectionRange(selection[0], selection[1]); } catch (_) {} } }
    }
    shown();
  }
  async function act(s, id, action, scope) {
    const context = {};
    for (const entry of Array.isArray(action.context) ? action.context : []) {
      if (entry && typeof entry.key === 'string') { const v = bound(entry.value, s, scope); context[entry.key] = v === undefined ? null : v; }
    }
    const body = JSON.stringify({ userAction: { name: String(action.name || ''), surfaceId: s.id, sourceComponentId: id, timestamp: new Date().toISOString(), context } });
    say('Sending…');
    try {
      const r = await fetch(base + '/action', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body });
      say(r.ok ? 'Sent to the agent.' : 'Not sent (' + r.status + ').');
    } catch (_) { say('Not sent: the agent is not running.'); }
  }
  function handle(data) {
    const touched = new Set();
    for (const line of String(data).split('\n')) {
      if (!line.trim()) continue; let m; try { m = JSON.parse(line); } catch (_) { continue; }
      const id = apply(m); if (id) touched.add(id);
    }
    for (const id of touched) { const s = surfaces.get(id); if (s) render(s); }
    shown();
  }
  const events = new EventSource(base + '/events');
  events.onopen = () => say('Live.');
  events.onerror = () => say('Reconnecting…');
  events.addEventListener('snapshot', (e) => { for (const s of surfaces.values()) s.el.remove(); surfaces.clear(); handle(e.data); });
  events.onmessage = (e) => handle(e.data);
})();
</script></body></html>
"##;

#[cfg(test)]
mod tests {
    use super::*;

    fn invoices() -> Vec<Value> {
        serde_json::from_str(
            r#"[
          {"surfaceUpdate": {"surfaceId": "main", "components": [
            {"id": "root", "component": {"Column": {"children": {"explicitList": ["title", "rows", "pay"]}}}},
            {"id": "title", "component": {"Text": {"text": {"literalString": "Invoices"}, "usageHint": "h2"}}},
            {"id": "rows", "component": {"List": {"children": {"template": {"dataBinding": "/owed", "componentId": "row"}}}}},
            {"id": "row", "component": {"Text": {"text": {"path": "line"}}}},
            {"id": "pay", "component": {"Button": {"child": "pay-label", "primary": true,
              "action": {"name": "pay", "context": [{"key": "note", "value": {"path": "/note"}}]}}}},
            {"id": "pay-label", "component": {"Text": {"text": {"literalString": "Pay both"}}}}
          ]}},
          {"dataModelUpdate": {"surfaceId": "main", "path": "/owed", "contents": [
            {"key": "a", "valueMap": [{"key": "line", "valueString": "1042 Brightside Studio 840.50"}]},
            {"key": "b", "valueMap": [{"key": "line", "valueString": "1043 Acme Ltd 400.00"}]}
          ]}},
          {"beginRendering": {"surfaceId": "main", "root": "root"}}
        ]"#,
        )
        .unwrap()
    }

    #[test]
    fn messages_are_applied_all_or_none_and_shown_again_whole() {
        let mut board = Board::default();
        let shown = board.apply(&invoices()).unwrap();
        assert_eq!(shown.len(), 3);
        let main = &board.surfaces["main"];
        assert_eq!(main.components.len(), 6);
        assert_eq!(main.root.as_deref(), Some("root"));
        assert_eq!(main.data["owed"]["b"]["line"], "1043 Acme Ltd 400.00");
        // What someone who starts watching is sent shows the same board.
        let mut again = Board::default();
        again.apply(&board.snapshot()).unwrap();
        assert_eq!(again, board);
        // A bad message leaves the board as it was.
        let before = board.clone();
        let refused = board
            .apply(&[
                json!({"deleteSurface": {"surfaceId": "main"}}),
                json!({"surfaceUpdate": {"components": [{"id": "x", "component": {"Script": {}}}]}}),
            ])
            .unwrap_err();
        assert!(
            refused.contains("not a component the canvas shows"),
            "{refused}"
        );
        assert_eq!(board, before);
    }

    #[test]
    fn what_could_reach_another_site_or_overflow_is_refused() {
        let mut board = Board::default();
        let image = |url: &str| {
            json!({"surfaceUpdate": {"components": [
                {"id": "p", "component": {"Image": {"url": {"literalString": url}}}}
            ]}})
        };
        assert!(board
            .apply(&[image("https://example.com/x.png?secret=1")])
            .unwrap_err()
            .contains("not shown from another site"));
        assert!(board
            .apply(&[image("data:image/png;base64,iVBORw0KGgo=")])
            .is_ok());
        let cases = [
            json!({"beginRendering": {"root": "nothing"}}),
            json!({"surfaceUpdate": {"components": [{"id": "has space", "component": {"Text": {}}}]}}),
            json!({"surfaceUpdate": {"components": [{"id": "b", "component": {"Button": {"child": "x"}}}]}}),
            json!({"dataModelUpdate": {"contents": [{"key": "a/b", "valueString": "x"}]}}),
            json!({"dataModelUpdate": {"contents": [{"key": "a"}]}}),
            json!({"render": {}}),
            json!({"surfaceUpdate": {}, "beginRendering": {}}),
            json!({"surfaceUpdate": {"components": [{"id": "t", "component": {"Text": {
                "text": {"literalString": "x".repeat(MAX_STRING + 1)}}}}]}}),
        ];
        for case in cases {
            assert!(board.apply(std::slice::from_ref(&case)).is_err(), "{case}");
        }
        let many: Vec<Value> = (0..=MAX_SURFACES)
            .map(|i| json!({"surfaceUpdate": {"surfaceId": format!("s{i}"), "components": []}}))
            .collect();
        assert!(board.apply(&many).unwrap_err().contains("at most"));
    }

    #[test]
    fn a_persons_action_is_taken_only_as_the_buttons_own() {
        let mut board = Board::default();
        board.apply(&invoices()).unwrap();
        let sent = |name: &str, source: &str| {
            json!({"userAction": {"name": name, "surfaceId": "main", "sourceComponentId": source,
                "timestamp": "2026-11-07T10:00:00Z", "context": {"note": "ok"}}})
        };
        let action = board.check_action(&sent("pay", "pay")).unwrap();
        assert_eq!(action["context"]["note"], "ok");
        assert!(board.check_action(&sent("refund", "pay")).is_err());
        assert!(board.check_action(&sent("pay", "title")).is_err());
        assert!(board.check_action(&json!({"name": "pay"})).is_err());
        assert_eq!(
            actions_report(&[action]),
            "the person pressed pay (pay on main), with {\"note\":\"ok\"}"
        );
    }

    fn ask(port: u16, request: &str) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut answer = String::new();
        let _ = stream.read_to_string(&mut answer);
        answer
    }

    #[test]
    fn the_page_is_served_by_its_token_watched_live_and_acted_on() {
        let canvas = Canvas::new();
        let shown = canvas.show(&invoices()).unwrap();
        assert!(
            shown.contains("main: 6 components, shown from root"),
            "{shown}"
        );
        let url = canvas.url().unwrap();
        let (port, token) = {
            let rest = url.trim_start_matches("http://127.0.0.1:");
            let (port, token) = rest.split_once('/').unwrap();
            (port.parse::<u16>().unwrap(), token.to_string())
        };
        let host = format!("127.0.0.1:{port}");
        let page = ask(
            port,
            &format!("GET /{token} HTTP/1.1\r\nHost: {host}\r\n\r\n"),
        );
        assert!(page.starts_with("HTTP/1.1 200 OK"), "{page}");
        assert!(page.contains("img-src data:; connect-src 'self'"));
        let nonce = &canvas.inner.nonce;
        assert!(page.contains(&format!("<script nonce=\"{nonce}\">")));
        assert!(ask(
            port,
            &format!("GET /{token} HTTP/1.1\r\nHost: evil.example\r\n\r\n")
        )
        .starts_with("HTTP/1.1 403"));
        assert!(ask(
            port,
            &format!("GET /wrong HTTP/1.1\r\nHost: {host}\r\n\r\n")
        )
        .starts_with("HTTP/1.1 404"));

        // Watching: the board as it is, then what is shown after.
        let mut watcher = TcpStream::connect(("127.0.0.1", port)).unwrap();
        watcher
            .write_all(format!("GET /{token}/events HTTP/1.1\r\nHost: {host}\r\n\r\n").as_bytes())
            .unwrap();
        watcher
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut reader = BufReader::new(watcher);
        let mut seen = String::new();
        while !seen.contains("\n\n") || !seen.contains("event: snapshot") {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            seen.push_str(&line);
        }
        assert!(seen.contains("text/event-stream"), "{seen}");
        assert!(seen.contains("\"Pay both\""), "{seen}");
        for _ in 0..50 {
            if canvas.inner.watching.load(Ordering::SeqCst) == 1 {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let shown = canvas
            .show(&[json!({"dataModelUpdate": {"path": "/owed", "contents": [
                {"key": "b", "valueMap": [{"key": "line", "valueString": "1043 Acme Ltd paid"}]}]}})])
            .unwrap();
        assert!(shown.contains("1 person is watching"), "{shown}");
        let mut update = String::new();
        while !update.contains("paid") {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            update.push_str(&line);
        }
        assert!(update.starts_with("data: {\"dataModelUpdate\""), "{update}");

        // The person presses the button; the agent hears it.
        let body = json!({"userAction": {"name": "pay", "surfaceId": "main",
            "sourceComponentId": "pay", "timestamp": "2026-11-07T10:00:00Z",
            "context": {"note": "both today"}}})
        .to_string();
        let pressed = ask(
            port,
            &format!(
                "POST /{token}/action HTTP/1.1\r\nHost: {host}\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            ),
        );
        assert!(pressed.starts_with("HTTP/1.1 204"), "{pressed}");
        let invented = json!({"userAction": {"name": "refund", "surfaceId": "main",
            "sourceComponentId": "pay"}})
        .to_string();
        let refused = ask(
            port,
            &format!(
                "POST /{token}/action HTTP/1.1\r\nHost: {host}\r\nContent-Length: {}\r\n\r\n{invented}",
                invented.len()
            ),
        );
        assert!(refused.starts_with("HTTP/1.1 400"), "{refused}");
        let tools = canvas.tools();
        let wait = tools
            .iter()
            .find(|t| t.spec().name == "canvas_wait")
            .unwrap();
        assert!(wait.waits_for_person());
        let heard = wait.call(&json!({"seconds": 2}), "k");
        assert_eq!(
            heard.content,
            "the person pressed pay (pay on main), with {\"note\":\"both today\"}"
        );
        assert!(!heard.untrusted);
        let nothing = wait.call(&json!({"seconds": 1}), "k");
        assert_eq!(
            nothing.content,
            "the person has done nothing on the canvas yet"
        );
        let show = tools
            .iter()
            .find(|t| t.spec().name == "canvas_show")
            .unwrap();
        assert_eq!(show.effect(), Effect::Write);
        assert!(
            show.call(&json!({"messages": [{"render": {}}]}), "k")
                .is_error
        );
    }
}
