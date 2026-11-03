//! The Chrome DevTools Protocol over a pipe, the way a browser started with
//! `--remote-debugging-pipe` speaks it: JSON messages ended by a NUL byte,
//! commands on the browser's file descriptor 3, answers and events on 4.
//!
//! The browser is started through `/bin/sh`, which puts its standard input
//! and output at those descriptors, so no port is opened and no other
//! program on the machine can drive the browser.
//!
//! A JavaScript dialog (`alert`, `confirm`, `prompt`) would hold its page
//! until answered: it is dismissed as it opens (a page's question whether
//! to leave it is answered yes), and its event is kept for the reader to
//! say what the page asked.

use std::collections::{BTreeMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// The longest message read, in bytes.
pub const MAX_MESSAGE: usize = 64 << 20;

/// The most events kept for a reader to look at.
const MAX_EVENTS: usize = 4096;

/// The shell command that starts a browser speaking the protocol on its
/// standard input and output: they become its descriptors 3 and 4, and its
/// own standard streams are closed off.
pub const PIPE_SHELL: &str =
    "exec 3<&0 4>&1 0</dev/null 1>/dev/null 2>/dev/null; exec \"$0\" \"$@\"";

type Pending = Arc<Mutex<BTreeMap<u64, mpsc::Sender<Value>>>>;

type Writer = Arc<Mutex<Box<dyn Write + Send>>>;

/// A connection to a browser over its pipe.
pub struct Cdp {
    writer: Writer,
    pending: Pending,
    events: Arc<(Mutex<VecDeque<Value>>, Condvar)>,
    next: Arc<AtomicU64>,
    timeout: Duration,
}

/// Writes one message to the browser.
fn write(writer: &Writer, message: &Value) -> std::io::Result<()> {
    let mut writer = writer.lock().unwrap_or_else(|p| p.into_inner());
    let mut bytes = message.to_string().into_bytes();
    bytes.push(0);
    writer.write_all(&bytes).and_then(|()| writer.flush())
}

impl std::fmt::Debug for Cdp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cdp")
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl Cdp {
    /// A connection writing commands to `writer` and reading answers and
    /// events from `reader`, each command waiting at most `timeout`.
    pub fn new(
        writer: impl Write + Send + 'static,
        reader: impl Read + Send + 'static,
        timeout: Duration,
    ) -> Cdp {
        Cdp::start(writer, reader, timeout, true)
    }

    /// [`Cdp::new`] for a browser a person uses: the dialogs its pages open
    /// are left for the person to answer.
    pub fn for_person(
        writer: impl Write + Send + 'static,
        reader: impl Read + Send + 'static,
        timeout: Duration,
    ) -> Cdp {
        Cdp::start(writer, reader, timeout, false)
    }

    fn start(
        writer: impl Write + Send + 'static,
        reader: impl Read + Send + 'static,
        timeout: Duration,
        dismiss_dialogs: bool,
    ) -> Cdp {
        let pending: Pending = Arc::new(Mutex::new(BTreeMap::new()));
        let events = Arc::new((Mutex::new(VecDeque::new()), Condvar::new()));
        let writer: Writer = Arc::new(Mutex::new(Box::new(writer)));
        let next = Arc::new(AtomicU64::new(0));
        let (routes, queue) = (pending.clone(), events.clone());
        let (answers, ids) = (writer.clone(), next.clone());
        std::thread::spawn(move || {
            let mut reader = BufReader::new(reader);
            loop {
                let mut raw = Vec::new();
                match reader.read_until(0, &mut raw) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                if raw.last() == Some(&0) {
                    raw.pop();
                }
                if raw.len() > MAX_MESSAGE {
                    continue;
                }
                let Ok(message) = serde_json::from_slice::<Value>(&raw) else {
                    continue;
                };
                match message["id"].as_u64() {
                    Some(id) => {
                        if let Some(sender) =
                            routes.lock().unwrap_or_else(|p| p.into_inner()).remove(&id)
                        {
                            let _ = sender.send(message);
                        }
                    }
                    None => {
                        if dismiss_dialogs && message["method"] == "Page.javascriptDialogOpening" {
                            let leaving = message["params"]["type"] == "beforeunload";
                            let mut answer = json!({
                                "id": ids.fetch_add(1, Ordering::SeqCst) + 1,
                                "method": "Page.handleJavaScriptDialog",
                                "params": {"accept": leaving},
                            });
                            if let Some(session) = message.get("sessionId") {
                                answer["sessionId"] = session.clone();
                            }
                            let _ = write(&answers, &answer);
                        }
                        let (lock, ready) = &*queue;
                        let mut events = lock.lock().unwrap_or_else(|p| p.into_inner());
                        if events.len() >= MAX_EVENTS {
                            events.pop_front();
                        }
                        events.push_back(message);
                        ready.notify_all();
                    }
                }
            }
            // The browser is gone: whoever waits is told.
            routes.lock().unwrap_or_else(|p| p.into_inner()).clear();
            queue.1.notify_all();
        });
        Cdp {
            writer,
            pending,
            events,
            next,
            timeout,
        }
    }

    /// Sends `method` with `params` (to the page attached as `session`, if
    /// one is given) and waits for its result.
    pub fn send(
        &self,
        session: Option<&str>,
        method: &str,
        params: Value,
    ) -> Result<Value, String> {
        self.send_waiting(session, method, params, self.timeout)
    }

    /// [`Cdp::send`], waiting at most `wait`.
    pub fn send_waiting(
        &self,
        session: Option<&str>,
        method: &str,
        params: Value,
        wait: Duration,
    ) -> Result<Value, String> {
        let id = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        let mut message = json!({"id": id, "method": method, "params": params});
        if let Some(session) = session {
            message["sessionId"] = json!(session);
        }
        let (sender, answer) = mpsc::channel();
        self.pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(id, sender);
        write(&self.writer, &message).map_err(|e| format!("the browser stopped: {e}"))?;
        let answer = answer.recv_timeout(wait).map_err(|_| {
            self.pending
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&id);
            format!(
                "the browser did not answer {method} within {} s",
                wait.as_secs()
            )
        })?;
        if let Some(error) = answer.get("error") {
            return Err(format!(
                "{method}: {}",
                error["message"]
                    .as_str()
                    .unwrap_or("the browser refused it")
            ));
        }
        Ok(answer["result"].clone())
    }

    /// Takes the events seen so far that `keep` selects, leaving the rest.
    pub fn take(&self, keep: impl Fn(&Value) -> bool) -> Vec<Value> {
        let (lock, _) = &*self.events;
        let mut events = lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut taken = Vec::new();
        events.retain(|event| {
            if keep(event) {
                taken.push(event.clone());
                false
            } else {
                true
            }
        });
        taken
    }

    /// Waits at most `wait` for an event that `wanted` selects, and takes
    /// it.
    pub fn wait_for(&self, wanted: impl Fn(&Value) -> bool, wait: Duration) -> Option<Value> {
        let deadline = Instant::now() + wait;
        let (lock, ready) = &*self.events;
        let mut events = lock.lock().unwrap_or_else(|p| p.into_inner());
        loop {
            if let Some(at) = events.iter().position(&wanted) {
                return events.remove(at);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            events = ready
                .wait_timeout(events, left)
                .unwrap_or_else(|p| p.into_inner())
                .0;
        }
    }

    /// Forgets the events seen so far.
    pub fn clear(&self) {
        let (lock, _) = &*self.events;
        lock.lock().unwrap_or_else(|p| p.into_inner()).clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in browser: answers each command with its method's name, and
    /// sends an event first.
    #[test]
    fn commands_are_answered_and_events_kept() {
        let (to_browser, from_client) = std::os::unix::net::UnixStream::pair().unwrap();
        let (to_client, from_browser) = std::os::unix::net::UnixStream::pair().unwrap();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(from_client);
            let mut writer = to_client;
            loop {
                let mut raw = Vec::new();
                if reader.read_until(0, &mut raw).unwrap_or(0) == 0 {
                    break;
                }
                raw.pop();
                let asked: Value = serde_json::from_slice(&raw).unwrap();
                let event = json!({"method": "Page.loadEventFired", "params": {}, "sessionId": asked["sessionId"]});
                let mut out = event.to_string().into_bytes();
                out.push(0);
                let answer = if asked["method"] == "Fail.now" {
                    json!({"id": asked["id"], "error": {"message": "no such method"}})
                } else {
                    json!({"id": asked["id"], "result": {"method": asked["method"]}})
                };
                out.extend(answer.to_string().into_bytes());
                out.push(0);
                writer.write_all(&out).unwrap();
            }
        });
        let cdp = Cdp::new(to_browser, from_browser, Duration::from_secs(5));
        let result = cdp
            .send(Some("s1"), "Page.navigate", json!({"url": "about:blank"}))
            .unwrap();
        assert_eq!(result["method"], "Page.navigate");
        let event = cdp
            .wait_for(
                |e| e["method"] == "Page.loadEventFired",
                Duration::from_secs(5),
            )
            .unwrap();
        assert_eq!(event["sessionId"], "s1");
        let err = cdp.send(None, "Fail.now", json!({})).unwrap_err();
        assert!(err.contains("no such method"), "{err}");
        assert!(cdp
            .wait_for(|e| e["method"] == "Nothing", Duration::from_millis(50))
            .is_none());
    }
}
