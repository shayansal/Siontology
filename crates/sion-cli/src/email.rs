//! Email: the agent's own mailbox, read over IMAP and answered over SMTP.
//! Each correspondent is a chat, named by their address; a reply goes to
//! them alone, in the thread of what they wrote.
//!
//! | Variable | What it is |
//! |---|---|
//! | `EMAIL_ADDRESS` | the agent's address |
//! | `EMAIL_IMAP_URL` | `imaps://imap.example.org` (TLS, port 993), or `imap://…` (STARTTLS, 143) |
//! | `EMAIL_SMTP_URL` | `smtps://smtp.example.org` (TLS, 465), or `smtp://…` (STARTTLS, 587) |
//! | `EMAIL_PASSWORD` | the password: an app password, where the provider gives them |
//! | `EMAIL_TOKEN_COMMAND` | instead of a password, a command that prints an OAuth access token (XOAUTH2) |
//! | `EMAIL_USERNAME` | the login, if it is not the address |
//! | `EMAIL_MAILBOX` | the mailbox read (`INBOX`) |
//! | `EMAIL_NAME` | the name replies are sent under |
//! | `EMAIL_AUTHSERV_ID` | the name the provider signs its `Authentication-Results` with |
//! | `EMAIL_TRUST_FROM` | `1`: take `From` as it is, for a server that checks nothing |
//!
//! **Who wrote it.** Anyone can write any `From`. A message is taken as
//! from its `From` address only when the mail provider's
//! `Authentication-Results` (RFC 8601) say so: DMARC passed for its
//! domain, or DKIM or SPF passed for a domain aligned with it. The results
//! read are the topmost, which the provider adds above everything the
//! sender wrote; with `EMAIL_AUTHSERV_ID`, the topmost the provider signs
//! with that name (`mx.google.com`; a domain stands for its servers:
//! `messagingengine.com`). Anything else is not answered (trust atom
//! `agent:email-from`).
//!
//! **What is not answered**: automatic mail (`Auto-Submitted`, bounces and
//! delivery reports, mailing lists, `Precedence: bulk`), so that two
//! machines never answer each other, and the agent's own. Replies are
//! marked `Auto-Submitted: auto-replied` (RFC 3834).
//!
//! **TLS.** Nothing is sent in the clear: an `imap://` or `smtp://` server
//! must offer STARTTLS, unless it is on a loopback address (a bridge on
//! the machine itself, as Proton Mail's is). Anything the server sends
//! between its go-ahead and the handshake is refused, as STARTTLS
//! injection would send it.
//!
//! It reads without changing the mailbox (no flag is set): the bridge
//! keeps its place by UID and, the first time, starts after the latest
//! message. It waits for mail with IDLE where the server has it, and asks
//! again every few seconds where not.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::{Read, Write};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig, STANDARD};
use base64::engine::DecodePaddingMode;
use base64::Engine;

use crate::live::{again, Fault};
use crate::messengers::{Incoming, Messenger, Note};
use crate::voice::Audio;

/// The longest message read whole; of a longer one, only the header.
pub const MAX_MESSAGE: u64 = 10 << 20;

/// The longest line read from a server, outside a literal.
const MAX_LINE: usize = 1 << 20;

/// The most messages one batch takes.
const BATCH: usize = 20;

/// The most of one message's text given to the agent, in characters.
const MAX_TEXT: usize = 16_000;

/// How long a server has to answer.
const ANSWER: Duration = Duration::from_secs(60);

/// How often a server without IDLE is asked for new mail while waiting.
const POLL: Duration = Duration::from_secs(10);

/// Base64 as mail writes it: padded or not, broken into lines.
const LENIENT: GeneralPurpose = GeneralPurpose::new(
    &base64::alphabet::STANDARD,
    GeneralPurposeConfig::new()
        .with_decode_padding_mode(DecodePaddingMode::RequireNone)
        .with_decode_allow_trailing_bits(true),
);

fn fatal(why: impl std::fmt::Display) -> Fault {
    Fault::Fatal(why.to_string())
}

fn reason(fault: Fault) -> String {
    match fault {
        Fault::Fatal(why) | Fault::Again(why) => why,
    }
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// How the agent logs in.
#[derive(Clone)]
enum Secret {
    Password(String),
    /// A command printing an OAuth access token.
    Token(String),
}

#[derive(Clone)]
struct Config {
    /// The agent's address, lower-cased.
    address: String,
    name: Option<String>,
    username: String,
    secret: Secret,
    imap: String,
    smtp: String,
    mailbox: String,
    authserv: Option<String>,
    trust_from: bool,
}

/// The thread a reply to a chat goes in.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Thread {
    subject: String,
    message_id: Option<String>,
    references: Vec<String>,
}

/// The agent's mailbox (see the module's documentation).
pub struct Email {
    config: Config,
    session: Mutex<Option<Imap>>,
    /// Faults in a row, for the pause before connecting again.
    failures: Mutex<u32>,
    threads: Mutex<BTreeMap<String, Thread>>,
    /// The audio of the last batch's voice notes, by note id.
    voices: Mutex<BTreeMap<String, Audio>>,
    /// The Message-IDs answered lately: a message delivered twice is
    /// answered once.
    answered: Mutex<VecDeque<String>>,
}

impl std::fmt::Debug for Email {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Email({})", self.config.address)
    }
}

impl Email {
    /// The mailbox the environment names (see the module's documentation).
    pub fn from_env() -> Result<Email, String> {
        let address = env("EMAIL_ADDRESS")
            .ok_or("set EMAIL_ADDRESS (the agent's own address)")?
            .to_lowercase();
        if !valid_address(&address) {
            return Err(format!("EMAIL_ADDRESS: {address} is not an address"));
        }
        let imap = env("EMAIL_IMAP_URL").ok_or("set EMAIL_IMAP_URL (imaps://imap.example.org)")?;
        let smtp = env("EMAIL_SMTP_URL").ok_or(
            "set EMAIL_SMTP_URL (smtps://smtp.example.org, or smtp://… for STARTTLS on port 587)",
        )?;
        for (name, url, schemes) in [
            ("EMAIL_IMAP_URL", &imap, ["imap", "imaps"]),
            ("EMAIL_SMTP_URL", &smtp, ["smtp", "smtps"]),
        ] {
            let scheme = url.split("://").next().unwrap_or_default();
            if !url.contains("://") || !schemes.iter().any(|s| scheme.eq_ignore_ascii_case(s)) {
                return Err(format!(
                    "{name} is {url}: an {}:// or {}:// URL",
                    schemes[0], schemes[1]
                ));
            }
        }
        let secret = match (env("EMAIL_PASSWORD"), env("EMAIL_TOKEN_COMMAND")) {
            (Some(_), Some(_)) => {
                return Err("set EMAIL_PASSWORD or EMAIL_TOKEN_COMMAND, not both".into())
            }
            (Some(password), None) => Secret::Password(password),
            (None, Some(command)) => Secret::Token(command),
            (None, None) => return Err(
                "set EMAIL_PASSWORD (an app password, where the provider gives them) or EMAIL_TOKEN_COMMAND (a command that prints an OAuth access token)"
                    .into(),
            ),
        };
        let mailbox = env("EMAIL_MAILBOX").unwrap_or_else(|| "INBOX".into());
        if !mailbox.bytes().all(|b| (b' '..=b'~').contains(&b)) {
            return Err(format!(
                "EMAIL_MAILBOX: {mailbox}: only mailboxes named in ASCII"
            ));
        }
        Ok(Email {
            config: Config {
                username: env("EMAIL_USERNAME").unwrap_or_else(|| address.clone()),
                address,
                name: env("EMAIL_NAME"),
                secret,
                imap,
                smtp,
                mailbox,
                authserv: env("EMAIL_AUTHSERV_ID"),
                trust_from: env("EMAIL_TRUST_FROM")
                    .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true")),
            },
            session: Mutex::new(None),
            failures: Mutex::new(0),
            threads: Mutex::new(BTreeMap::new()),
            voices: Mutex::new(BTreeMap::new()),
            answered: Mutex::new(VecDeque::new()),
        })
    }

    /// The new messages after `cursor`, waiting until `deadline` for one.
    fn read(
        &self,
        session: &mut Option<Imap>,
        cursor: &str,
        deadline: Instant,
    ) -> Result<(Vec<Incoming>, String), Fault> {
        if session.is_none() {
            *session = Some(Imap::open(&self.config)?);
        }
        let imap = session.as_mut().expect("just opened");
        // Where to go on from: the cursor, in the same mailbox; after the
        // latest message, the first time or in a mailbox made anew.
        let last = match cursor.split_once(':') {
            Some((validity, last)) if validity.parse() == Ok(imap.validity) => {
                last.parse().unwrap_or(0)
            }
            _ => imap.next.saturating_sub(1),
        };
        loop {
            let found = imap.new_since(last)?;
            if !found.is_empty() {
                let mut messages = Vec::new();
                let mut upto = last;
                for (uid, size) in found.into_iter().take(BATCH) {
                    let whole = size <= MAX_MESSAGE;
                    let raw = imap.message(uid, whole)?;
                    upto = uid;
                    if let Some(message) = raw.and_then(|raw| self.incoming(uid, &raw, whole, size))
                    {
                        messages.push(message);
                    }
                }
                return Ok((messages, format!("{}:{upto}", imap.validity)));
            }
            if Instant::now() >= deadline {
                return Ok((Vec::new(), format!("{}:{last}", imap.validity)));
            }
            imap.wait(deadline)?;
        }
    }

    /// The message `raw` as the bridge takes it, if it is to be answered.
    fn incoming(&self, uid: u64, raw: &[u8], whole: bool, size: u64) -> Option<Incoming> {
        let mail = parse(raw);
        if mail.from.is_empty() {
            println!("email {uid}: no sender, ignored");
            return None;
        }
        if mail.from == self.config.address {
            return None;
        }
        if let Some(why) = &mail.automatic {
            println!("address {}: {why}, ignored", mail.from);
            return None;
        }
        if !self.config.trust_from {
            if let Err(why) =
                authenticated(&mail.results, &mail.from, self.config.authserv.as_deref())
            {
                println!("address {}: {why}, ignored", mail.from);
                return None;
            }
        }
        if let Some(id) = &mail.message_id {
            let mut answered = self.answered.lock().unwrap_or_else(|e| e.into_inner());
            if answered.contains(id) {
                return None;
            }
            answered.push_back(id.clone());
            while answered.len() > 500 {
                answered.pop_front();
            }
        }
        let mut references = mail.references.clone();
        if let Some(id) = &mail.message_id {
            references.push(id.clone());
        }
        let skip = references.len().saturating_sub(10);
        self.threads
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                mail.from.clone(),
                Thread {
                    subject: mail.subject.clone(),
                    message_id: mail.message_id.clone(),
                    references: references.split_off(skip),
                },
            );
        let voice = mail.audio.first().map(|(media_type, name, bytes)| {
            let id = uid.to_string();
            self.voices
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(
                    id.clone(),
                    Audio {
                        media_type: media_type.clone(),
                        name: name.clone(),
                        bytes: bytes.clone(),
                    },
                );
            Note {
                id,
                media_type: Some(media_type.clone()),
            }
        });
        let mut text = mail.said();
        if !whole {
            text = format!(
                "(a message of {} MB, too long to be read whole: only its subject was read) {text}",
                size.div_ceil(1 << 20)
            );
        }
        let others = mail
            .to
            .iter()
            .chain(&mail.cc)
            .any(|a| *a != self.config.address);
        Some(Incoming {
            chat: mail.from.clone(),
            from: mail.from,
            text,
            voice,
            direct: !others,
            // Being written to, not copied, is being spoken to.
            mentions: mail.to,
        })
    }

    /// Sends `message` to `to`, once more if the server could not be
    /// reached; not again once the server may have it.
    fn submit(&self, to: &str, message: &[u8]) -> Result<(), String> {
        match submit(&self.config, to, message) {
            Ok(()) => Ok(()),
            Err(Fault::Again(why)) => {
                println!("email: {why}; sending again in 2 s");
                std::thread::sleep(Duration::from_secs(2));
                submit(&self.config, to, message).map_err(|f| format!("email: {}", reason(f)))
            }
            Err(Fault::Fatal(why)) => Err(format!("email: {why}")),
        }
    }

    /// A message to `to` saying `text`, with `audio` attached, in the
    /// thread of what `to` wrote last.
    fn compose(&self, to: &str, text: &str, audio: Option<&Audio>) -> Vec<u8> {
        let thread = self
            .threads
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(to)
            .cloned();
        compose(
            &self.config.address,
            self.config.name.as_deref(),
            to,
            thread.as_ref(),
            text,
            audio,
            now(),
            &random_hex(16),
        )
    }
}

impl Drop for Email {
    fn drop(&mut self) {
        // The bridge is done: the mailbox is logged out of.
        let session = self
            .session
            .get_mut()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(mut imap) = session {
            let _ = imap.command("LOGOUT");
        }
    }
}

impl Messenger for Email {
    fn name(&self) -> &'static str {
        "email"
    }

    fn chat_word(&self) -> &'static str {
        "address"
    }

    fn receive(&self, cursor: &str, wait: u64) -> Result<(Vec<Incoming>, String), String> {
        let deadline = Instant::now() + Duration::from_secs(wait);
        self.voices
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        let mut session = self.session.lock().unwrap_or_else(|e| e.into_inner());
        let reused = session.is_some();
        let mut read = self.read(&mut session, cursor, deadline);
        if reused && matches!(read, Err(Fault::Again(_))) {
            // A connection kept from before may have been closed meanwhile.
            *session = None;
            read = self.read(&mut session, cursor, deadline);
        }
        let mut failures = self.failures.lock().unwrap_or_else(|e| e.into_inner());
        match read {
            Ok(found) => {
                *failures = 0;
                Ok(found)
            }
            Err(Fault::Fatal(why)) => {
                *session = None;
                Err(format!("email: {why}"))
            }
            Err(Fault::Again(why)) => {
                *session = None;
                if wait == 0 {
                    return Err(format!("email: {why}"));
                }
                *failures += 1;
                let pause = Duration::from_secs(1u64 << (*failures).min(6))
                    .min(Duration::from_secs(60))
                    .min(Duration::from_secs(wait));
                println!("email: {why}; connecting again in {} s", pause.as_secs());
                std::thread::sleep(pause);
                Ok((Vec::new(), cursor.to_string()))
            }
        }
    }

    fn send(&self, chat: &str, text: &str) -> Result<(), String> {
        if !valid_address(chat) {
            return Err(format!("{chat} is not an email address"));
        }
        let message = self.compose(chat, text, None);
        self.submit(chat, &message)
    }

    fn long_polls(&self) -> bool {
        true
    }

    fn me(&self) -> Result<Vec<String>, String> {
        Ok(vec![self.config.address.clone()])
    }

    fn fetch_voice(&self, note: &Note) -> Result<Audio, String> {
        self.voices
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&note.id)
            .cloned()
            .ok_or_else(|| "that voice note is gone".to_string())
    }

    fn send_voice(&self, chat: &str, audio: &Audio, caption: &str) -> Result<bool, String> {
        if !valid_address(chat) {
            return Err(format!("{chat} is not an email address"));
        }
        let message = self.compose(chat, caption, Some(audio));
        self.submit(chat, &message).map(|()| true)
    }
}

// --- Connections ---------------------------------------------------------

/// Whether `host` is the machine itself, where a connection in the clear
/// crosses no network.
fn is_loopback(host: &str) -> bool {
    host == "localhost"
        || host.ends_with(".localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// The host of a URL, lower-cased, without brackets or port.
fn url_host(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = match authority.strip_prefix('[') {
        Some(bracketed) => bracketed.split(']').next()?,
        None => authority.rsplit_once(':').map_or(authority, |(h, _)| h),
    };
    Some(host.to_ascii_lowercase())
}

/// A client for `url`'s server: a server on the machine itself is reached
/// directly, never through a proxy.
fn client_for(url: &str) -> Result<sion_http::Client, Fault> {
    let local = url_host(url).is_some_and(|h| is_loopback(&h));
    sion_http::Client::new(sion_http::Options {
        connect_timeout: Duration::from_secs(10),
        timeout: ANSWER,
        // The person names their server.
        allow_private: true,
        proxy_from_env: !local,
        ..sion_http::Options::default()
    })
    .map_err(fatal)
}

/// A connection read a line or a run of bytes at a time.
struct Wire {
    stream: sion_http::Stream,
    buffer: Vec<u8>,
}

impl Wire {
    fn new(stream: sion_http::Stream) -> Wire {
        Wire {
            stream,
            buffer: Vec::new(),
        }
    }

    /// Reads what has come: `false` if nothing came in time.
    fn read_more(&mut self) -> Result<bool, Fault> {
        let mut chunk = [0u8; 16 << 10];
        match self.stream.read(&mut chunk) {
            Ok(0) => Err(again("the server closed the connection")),
            Ok(n) => {
                self.buffer.extend_from_slice(&chunk[..n]);
                Ok(true)
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                Ok(false)
            }
            Err(e) => Err(again(e)),
        }
    }

    /// The next line, without its line break, or `None` if none came in
    /// time.
    fn next_line(&mut self) -> Result<Option<Vec<u8>>, Fault> {
        loop {
            if let Some(end) = self.buffer.iter().position(|b| *b == b'\n') {
                let mut line: Vec<u8> = self.buffer.drain(..=end).collect();
                line.pop();
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                return Ok(Some(line));
            }
            if self.buffer.len() > MAX_LINE {
                return Err(again("a line longer than any server's"));
            }
            if !self.read_more()? {
                return Ok(None);
            }
        }
    }

    fn line(&mut self) -> Result<Vec<u8>, Fault> {
        self.next_line()?
            .ok_or_else(|| again("the server did not answer in time"))
    }

    fn bytes(&mut self, n: usize) -> Result<Vec<u8>, Fault> {
        while self.buffer.len() < n {
            if !self.read_more()? {
                return Err(again("the server did not answer in time"));
            }
        }
        Ok(self.buffer.drain(..n).collect())
    }

    fn send(&mut self, data: &[u8]) -> Result<(), Fault> {
        self.stream
            .write_all(data)
            .and_then(|()| self.stream.flush())
            .map_err(again)
    }

    /// The connection as TLS, once the server has agreed (STARTTLS).
    fn start_tls(self, client: &sion_http::Client) -> Result<Wire, Fault> {
        if !self.buffer.is_empty() {
            return Err(fatal(format!(
                "{} sent more after agreeing to TLS and before it: refused, as STARTTLS injection would send it",
                self.stream.host()
            )));
        }
        client.start_tls(self.stream).map(Wire::new).map_err(fatal)
    }

    /// Whether what is sent may be sent: over TLS, or to the machine
    /// itself.
    fn private(&self) -> bool {
        self.stream.is_tls() || is_loopback(self.stream.host())
    }
}

/// SASL PLAIN's initial response.
fn plain(username: &str, password: &str) -> String {
    STANDARD.encode(format!("\0{username}\0{password}"))
}

/// XOAUTH2's initial response (Google's and Microsoft's).
fn xoauth2(username: &str, token: &str) -> String {
    STANDARD.encode(format!("user={username}\x01auth=Bearer {token}\x01\x01"))
}

/// The access token `command` prints.
fn token(command: &str) -> Result<String, Fault> {
    let mut child = std::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .map_err(|e| fatal(format!("EMAIL_TOKEN_COMMAND: {e}")))?;
    let until = Instant::now() + Duration::from_secs(60);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < until => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(fatal("EMAIL_TOKEN_COMMAND did not finish within a minute"));
            }
        }
    }
    let output = child
        .wait_with_output()
        .map_err(|e| fatal(format!("EMAIL_TOKEN_COMMAND: {e}")))?;
    let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !output.status.success() || token.is_empty() || token.contains(char::is_whitespace) {
        return Err(fatal(
            "EMAIL_TOKEN_COMMAND printed no token (it should print the access token alone)",
        ));
    }
    Ok(token)
}

// --- IMAP ----------------------------------------------------------------

/// A line of a server's answer, with the literals it carried, each marked
/// `{#n}` in its text.
#[derive(Debug, Default)]
struct Said {
    text: String,
    literals: Vec<Vec<u8>>,
}

/// What a command was answered.
struct Answer {
    untagged: Vec<Said>,
    /// `OK`, `NO` or `BAD`.
    status: String,
    rest: String,
}

/// The size of the literal a line ends with (`{123}`, `{123+}`), and where
/// it starts.
fn literal_size(line: &str) -> Option<(u64, usize)> {
    let body = line.strip_suffix('}')?;
    let start = body.rfind('{')?;
    let digits = body[start + 1..].trim_end_matches('+');
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((digits.parse().ok()?, start))
}

/// The next line of what the server says, its literals read with it.
fn said(wire: &mut Wire) -> Result<Said, Fault> {
    let first = wire.line()?;
    said_after(wire, first)
}

fn said_after(wire: &mut Wire, first: Vec<u8>) -> Result<Said, Fault> {
    let mut said = Said::default();
    let mut line = first;
    loop {
        let text = String::from_utf8_lossy(&line).into_owned();
        match literal_size(&text) {
            Some((size, start)) => {
                if size > MAX_MESSAGE + (1 << 20) {
                    return Err(again(format!("the server sent a literal of {size} bytes")));
                }
                said.text.push_str(&text[..start]);
                said.text.push_str(&format!("{{#{}}}", said.literals.len()));
                said.literals.push(wire.bytes(size as usize)?);
                line = wire.line()?;
            }
            None => {
                said.text.push_str(&text);
                return Ok(said);
            }
        }
    }
}

/// A piece of an IMAP response.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Token {
    Open,
    Close,
    Atom(String),
    Quoted(String),
    /// The `n`th literal of the line.
    Literal(usize),
}

fn tokens(text: &str) -> Vec<Token> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            ' ' => i += 1,
            '(' => {
                out.push(Token::Open);
                i += 1;
            }
            ')' => {
                out.push(Token::Close);
                i += 1;
            }
            '"' => {
                let mut s = String::new();
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    if chars[i] == '\\' && i + 1 < chars.len() {
                        i += 1;
                    }
                    s.push(chars[i]);
                    i += 1;
                }
                i += 1;
                out.push(Token::Quoted(s));
            }
            '{' if chars.get(i + 1) == Some(&'#') => {
                let end = chars[i..].iter().position(|c| *c == '}').map(|p| i + p);
                let n =
                    end.and_then(|end| chars[i + 2..end].iter().collect::<String>().parse().ok());
                match (end, n) {
                    (Some(end), Some(n)) => {
                        out.push(Token::Literal(n));
                        i = end + 1;
                    }
                    _ => i += 1,
                }
            }
            _ => {
                // An atom, brackets and what they hold included (BODY[]).
                let start = i;
                let mut depth = 0u32;
                while i < chars.len() {
                    match chars[i] {
                        '[' => depth += 1,
                        ']' => depth = depth.saturating_sub(1),
                        ' ' | '(' | ')' if depth == 0 => break,
                        _ => {}
                    }
                    i += 1;
                }
                out.push(Token::Atom(chars[start..i].iter().collect()));
            }
        }
    }
    out
}

/// What a FETCH response gave.
#[derive(Debug, Default, PartialEq, Eq)]
struct Fetched {
    uid: Option<u64>,
    size: Option<u64>,
    /// `BODY[]`, or `BODY[HEADER]`.
    body: Option<Vec<u8>>,
}

/// The items of a FETCH response (`* 12 FETCH (UID 34 BODY[] {#0})`).
fn fetched(said: &Said) -> Option<Fetched> {
    let rest = said.text.strip_prefix("* ")?;
    let (_, rest) = rest.split_once(' ')?;
    let (word, rest) = rest.split_once(' ')?;
    if !word.eq_ignore_ascii_case("FETCH") {
        return None;
    }
    let tokens = tokens(rest);
    if tokens.first() != Some(&Token::Open) {
        return None;
    }
    let mut out = Fetched::default();
    let mut i = 1;
    while i + 1 < tokens.len() {
        let Token::Atom(name) = &tokens[i] else {
            break;
        };
        let name = name.to_ascii_uppercase();
        let value = &tokens[i + 1];
        let number = match value {
            Token::Atom(a) => a.parse().ok(),
            _ => None,
        };
        match name.as_str() {
            "UID" => out.uid = number,
            "RFC822.SIZE" => out.size = number,
            n if n.starts_with("BODY[") => {
                out.body = match value {
                    Token::Literal(n) => said.literals.get(*n).cloned(),
                    Token::Quoted(s) => Some(s.clone().into_bytes()),
                    _ => None,
                }
            }
            _ => {}
        }
        // Past the value, a list whole.
        i += 1;
        if tokens[i] == Token::Open {
            let mut depth = 0;
            while i < tokens.len() {
                match tokens[i] {
                    Token::Open => depth += 1,
                    Token::Close => depth -= 1,
                    _ => {}
                }
                i += 1;
                if depth == 0 {
                    break;
                }
            }
        } else {
            i += 1;
        }
    }
    Some(out)
}

/// The words of a capability list, upper-cased.
fn capability_words(text: &str) -> BTreeSet<String> {
    text.split_whitespace()
        .map(|w| w.to_ascii_uppercase())
        .collect()
}

/// The capabilities a response code gives (`[CAPABILITY IMAP4rev1 …]`).
fn bracketed_capabilities(text: &str) -> Option<BTreeSet<String>> {
    let start = text.to_ascii_uppercase().find("[CAPABILITY ")? + "[CAPABILITY ".len();
    let rest = &text[start..];
    Some(capability_words(&rest[..rest.find(']')?]))
}

/// The number a response code gives (`[UIDNEXT 42]`).
fn code_number(text: &str, code: &str) -> Option<u64> {
    let start = text.find(&format!("[{code} "))? + code.len() + 2;
    let rest = &text[start..];
    rest[..rest.find(']')?].trim().parse().ok()
}

/// An IMAP quoted string.
fn quoted(text: &str) -> Result<String, Fault> {
    if !text.bytes().all(|b| (b' '..=b'~').contains(&b)) {
        return Err(fatal(
            "a login or mailbox with characters outside ASCII: the server needs AUTH=PLAIN for it",
        ));
    }
    Ok(format!(
        "\"{}\"",
        text.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

/// A connection to the mailbox, logged in, the mailbox selected.
struct Imap {
    wire: Wire,
    tag: u32,
    capabilities: BTreeSet<String>,
    /// The mailbox's UIDVALIDITY: its UIDs mean the same while it lasts.
    validity: u64,
    /// The UID the next message will have.
    next: u64,
}

impl Imap {
    fn open(config: &Config) -> Result<Imap, Fault> {
        let client = client_for(&config.imap)?;
        let stream = client.connect(&config.imap).map_err(again)?;
        let host = stream.host().to_string();
        let mut imap = Imap {
            wire: Wire::new(stream),
            tag: 0,
            capabilities: BTreeSet::new(),
            validity: 0,
            next: 0,
        };
        let greeting = said(&mut imap.wire)?;
        let preauth = greeting.text.starts_with("* PREAUTH");
        if !greeting.text.starts_with("* OK") && !preauth {
            return Err(again(format!(
                "{host} greeted with: {}",
                greeting.text.chars().take(200).collect::<String>()
            )));
        }
        match bracketed_capabilities(&greeting.text) {
            Some(capabilities) => imap.capabilities = capabilities,
            None => imap.ask_capabilities()?,
        }
        if !imap.wire.stream.is_tls() {
            if imap.capabilities.contains("STARTTLS") {
                imap.ok("STARTTLS")?;
                let Imap { wire, tag, .. } = imap;
                imap = Imap {
                    wire: wire.start_tls(&client)?,
                    tag,
                    capabilities: BTreeSet::new(),
                    validity: 0,
                    next: 0,
                };
                // What was said in the clear counts for nothing.
                imap.ask_capabilities()?;
            } else if !imap.wire.private() {
                return Err(fatal(format!(
                    "{host} offers no STARTTLS: the login is not sent in the clear (use imaps://)"
                )));
            }
        }
        if !preauth {
            imap.login(config)?;
            imap.ask_capabilities()?;
        }
        imap.select(&config.mailbox)?;
        Ok(imap)
    }

    fn next_tag(&mut self) -> String {
        self.tag += 1;
        format!("s{}", self.tag)
    }

    fn command(&mut self, command: &str) -> Result<Answer, Fault> {
        let tag = self.next_tag();
        self.wire.send(format!("{tag} {command}\r\n").as_bytes())?;
        self.finish(&tag)
    }

    /// What the server says until it answers `tag`.
    fn finish(&mut self, tag: &str) -> Result<Answer, Fault> {
        let tagged = format!("{tag} ");
        let mut untagged = Vec::new();
        loop {
            let said = said(&mut self.wire)?;
            if let Some(rest) = said.text.strip_prefix(&tagged) {
                let (status, rest) = rest.split_once(' ').unwrap_or((rest, ""));
                return Ok(Answer {
                    untagged,
                    status: status.to_ascii_uppercase(),
                    rest: rest.to_string(),
                });
            }
            if let Some(why) = said.text.strip_prefix("* BYE") {
                return Err(again(format!("the server closed the connection:{why}")));
            }
            untagged.push(said);
        }
    }

    /// `command`, which must be answered OK.
    fn ok(&mut self, command: &str) -> Result<Answer, Fault> {
        let answer = self.command(command)?;
        if answer.status != "OK" {
            let word = command.split(' ').next().unwrap_or(command);
            return Err(fatal(format!(
                "the server answered {word} with {} {}",
                answer.status, answer.rest
            )));
        }
        Ok(answer)
    }

    fn ask_capabilities(&mut self) -> Result<(), Fault> {
        let answer = self.ok("CAPABILITY")?;
        for said in &answer.untagged {
            if let Some(rest) = said.text.strip_prefix("* CAPABILITY ") {
                self.capabilities = capability_words(rest);
            }
        }
        Ok(())
    }

    fn login(&mut self, config: &Config) -> Result<(), Fault> {
        match &config.secret {
            Secret::Token(command) => {
                if !self.capabilities.contains("AUTH=XOAUTH2") {
                    return Err(fatal("the server takes no OAuth token (AUTH=XOAUTH2)"));
                }
                let initial = xoauth2(&config.username, &token(command)?);
                self.authenticate("XOAUTH2", &initial)
            }
            Secret::Password(password) if self.capabilities.contains("AUTH=PLAIN") => {
                self.authenticate("PLAIN", &plain(&config.username, password))
            }
            Secret::Password(password) if !self.capabilities.contains("LOGINDISABLED") => {
                let answer = self.command(&format!(
                    "LOGIN {} {}",
                    quoted(&config.username)?,
                    quoted(password)?
                ))?;
                if answer.status != "OK" {
                    return Err(fatal(format!(
                        "the server refused the login: {}",
                        answer.rest
                    )));
                }
                Ok(())
            }
            Secret::Password(_) => Err(fatal(
                "the server offers no login made here (AUTH=PLAIN, LOGIN)",
            )),
        }
    }

    /// `AUTHENTICATE mechanism`, with `initial` as the first response.
    fn authenticate(&mut self, mechanism: &str, initial: &str) -> Result<(), Fault> {
        let tag = self.next_tag();
        let tagged = format!("{tag} ");
        let mut sent = self.capabilities.contains("SASL-IR");
        if sent {
            self.wire
                .send(format!("{tag} AUTHENTICATE {mechanism} {initial}\r\n").as_bytes())?;
        } else {
            self.wire
                .send(format!("{tag} AUTHENTICATE {mechanism}\r\n").as_bytes())?;
        }
        loop {
            let said = said(&mut self.wire)?;
            if said.text.starts_with('+') {
                if sent {
                    // An error's details (XOAUTH2's): an empty response ends
                    // the exchange, and the refusal follows.
                    self.wire.send(b"\r\n")?;
                } else {
                    self.wire.send(format!("{initial}\r\n").as_bytes())?;
                    sent = true;
                }
                continue;
            }
            if let Some(rest) = said.text.strip_prefix(&tagged) {
                let (status, rest) = rest.split_once(' ').unwrap_or((rest, ""));
                if status.eq_ignore_ascii_case("OK") {
                    return Ok(());
                }
                return Err(fatal(format!(
                    "the server refused the login ({mechanism}): {}",
                    rest.trim()
                )));
            }
        }
    }

    fn select(&mut self, mailbox: &str) -> Result<(), Fault> {
        let answer = self.command(&format!("SELECT {}", quoted(mailbox)?))?;
        if answer.status != "OK" {
            return Err(fatal(format!("the mailbox {mailbox}: {}", answer.rest)));
        }
        self.validity = 0;
        self.next = 0;
        for said in &answer.untagged {
            if let Some(validity) = code_number(&said.text, "UIDVALIDITY") {
                self.validity = validity;
            }
            if let Some(next) = code_number(&said.text, "UIDNEXT") {
                self.next = next;
            }
        }
        if self.next == 0 {
            // A server that does not say: after the highest UID there is.
            let answer = self.command("UID FETCH * (UID)")?;
            self.next = answer
                .untagged
                .iter()
                .filter_map(fetched)
                .filter_map(|f| f.uid)
                .max()
                .unwrap_or(0)
                + 1;
        }
        Ok(())
    }

    /// The UIDs after `last`, in order, with their sizes.
    fn new_since(&mut self, last: u64) -> Result<Vec<(u64, u64)>, Fault> {
        let answer = self.command(&format!("UID FETCH {}:* (UID RFC822.SIZE)", last + 1))?;
        match answer.status.as_str() {
            "OK" => {}
            // A server that says so of an empty mailbox.
            "NO" => return Ok(Vec::new()),
            _ => {
                return Err(fatal(format!(
                    "the server answered UID FETCH with {} {}",
                    answer.status, answer.rest
                )))
            }
        }
        let mut found: Vec<(u64, u64)> = answer
            .untagged
            .iter()
            .filter_map(fetched)
            .filter_map(|f| Some((f.uid?, f.size.unwrap_or(0))))
            .filter(|(uid, _)| *uid > last)
            .collect();
        found.sort_unstable();
        found.dedup_by_key(|(uid, _)| *uid);
        Ok(found)
    }

    /// The message `uid`, whole or its header only, without marking it
    /// read; `None` if it is gone.
    fn message(&mut self, uid: u64, whole: bool) -> Result<Option<Vec<u8>>, Fault> {
        let item = if whole {
            "BODY.PEEK[]"
        } else {
            "BODY.PEEK[HEADER]"
        };
        let answer = self.ok(&format!("UID FETCH {uid} ({item})"))?;
        Ok(answer
            .untagged
            .iter()
            .filter_map(fetched)
            .find(|f| f.body.is_some() && f.uid.map_or(true, |u| u == uid))
            .and_then(|f| f.body))
    }

    /// Waits until `deadline`, or until new mail may have come.
    fn wait(&mut self, deadline: Instant) -> Result<(), Fault> {
        if !self.capabilities.contains("IDLE") {
            std::thread::sleep(deadline.saturating_duration_since(Instant::now()).min(POLL));
            self.ok("NOOP")?;
            return Ok(());
        }
        let tag = self.next_tag();
        self.wire.send(format!("{tag} IDLE\r\n").as_bytes())?;
        loop {
            let said = said(&mut self.wire)?;
            if said.text.starts_with('+') {
                break;
            }
            if said.text.starts_with(&format!("{tag} ")) {
                // IDLE refused: asked every few seconds instead.
                self.capabilities.remove("IDLE");
                return Ok(());
            }
        }
        let mut news = false;
        while !news {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            self.wire
                .stream
                .set_read_timeout(Some(left.max(Duration::from_millis(20))))
                .map_err(again)?;
            let Some(first) = self.wire.next_line()? else {
                continue;
            };
            let said = said_after(&mut self.wire, first)?;
            let upper = said.text.to_ascii_uppercase();
            if upper.starts_with("* BYE") {
                return Err(again("the server closed the connection"));
            }
            news = upper.starts_with("* ") && upper.ends_with(" EXISTS");
        }
        self.wire
            .stream
            .set_read_timeout(Some(ANSWER))
            .map_err(again)?;
        self.wire.send(b"DONE\r\n")?;
        let answer = self.finish(&tag)?;
        if answer.status != "OK" {
            self.capabilities.remove("IDLE");
        }
        Ok(())
    }
}

// --- SMTP ----------------------------------------------------------------

/// An SMTP reply: its code and its lines' text.
fn reply(wire: &mut Wire) -> Result<(u16, Vec<String>), Fault> {
    let mut lines = Vec::new();
    loop {
        let line = String::from_utf8_lossy(&wire.line()?).into_owned();
        let code = line
            .get(..3)
            .and_then(|c| c.parse().ok())
            .ok_or_else(|| again(format!("the server said {line:?}")))?;
        let more = line.as_bytes().get(3) == Some(&b'-');
        lines.push(line.get(4..).unwrap_or_default().to_string());
        if !more {
            return Ok((code, lines));
        }
        if lines.len() > 1000 {
            return Err(again("an answer without end"));
        }
    }
}

/// Sends `command` (if any) and reads the reply, which must be one of
/// `codes`.
fn expect(
    wire: &mut Wire,
    command: Option<&str>,
    codes: &[u16],
    what: &str,
) -> Result<Vec<String>, Fault> {
    if let Some(command) = command {
        wire.send(format!("{command}\r\n").as_bytes())?;
    }
    let (code, lines) = reply(wire)?;
    if codes.contains(&code) {
        return Ok(lines);
    }
    let said = format!("{what}: the server answered {code} {}", lines.join(" "));
    Err(if code >= 500 {
        fatal(said)
    } else {
        again(said)
    })
}

/// The login mechanisms an EHLO reply offers.
fn mechanisms(extensions: &[String]) -> BTreeSet<String> {
    extensions
        .iter()
        .filter_map(|e| {
            let upper = e.to_ascii_uppercase();
            upper
                .strip_prefix("AUTH ")
                .or_else(|| upper.strip_prefix("AUTH="))
                .map(str::to_string)
        })
        .flat_map(|m| m.split_whitespace().map(str::to_string).collect::<Vec<_>>())
        .collect()
}

fn offers(extensions: &[String], name: &str) -> bool {
    extensions.iter().any(|e| {
        e.split_whitespace()
            .next()
            .is_some_and(|w| w.eq_ignore_ascii_case(name))
    })
}

/// Logs in to an SMTP server offering `mechanisms`.
fn smtp_login(
    wire: &mut Wire,
    config: &Config,
    mechanisms: &BTreeSet<String>,
) -> Result<(), Fault> {
    match &config.secret {
        Secret::Token(command) => {
            if !mechanisms.contains("XOAUTH2") {
                return Err(fatal("the SMTP server takes no OAuth token (AUTH XOAUTH2)"));
            }
            let initial = xoauth2(&config.username, &token(command)?);
            wire.send(format!("AUTH XOAUTH2 {initial}\r\n").as_bytes())?;
            let (mut code, mut lines) = reply(wire)?;
            if code == 334 {
                // The error's details; an empty response gets the refusal.
                wire.send(b"\r\n")?;
                (code, lines) = reply(wire)?;
            }
            if code != 235 {
                return Err(fatal(format!(
                    "the SMTP server refused the token: {code} {}",
                    lines.join(" ")
                )));
            }
        }
        Secret::Password(password) if mechanisms.contains("PLAIN") => {
            wire.send(format!("AUTH PLAIN {}\r\n", plain(&config.username, password)).as_bytes())?;
            expect(wire, None, &[235], "the login")?;
        }
        Secret::Password(password) if mechanisms.contains("LOGIN") => {
            expect(wire, Some("AUTH LOGIN"), &[334], "the login")?;
            wire.send(format!("{}\r\n", STANDARD.encode(config.username.as_bytes())).as_bytes())?;
            expect(wire, None, &[334], "the login")?;
            wire.send(format!("{}\r\n", STANDARD.encode(password.as_bytes())).as_bytes())?;
            expect(wire, None, &[235], "the login")?;
        }
        Secret::Password(_) => {
            return Err(fatal(
                "the SMTP server offers no login made here (AUTH PLAIN, LOGIN)",
            ))
        }
    }
    Ok(())
}

/// Submits `message` to `to` through the SMTP server.
fn submit(config: &Config, to: &str, message: &[u8]) -> Result<(), Fault> {
    let client = client_for(&config.smtp)?;
    let mut wire = Wire::new(client.connect(&config.smtp).map_err(again)?);
    let host = wire.stream.host().to_string();
    expect(&mut wire, None, &[220], "the greeting")?;
    let ehlo = format!("EHLO {}", domain_of(&config.address));
    let mut extensions = expect(&mut wire, Some(&ehlo), &[250], "EHLO")?;
    if !wire.stream.is_tls() {
        if offers(&extensions, "STARTTLS") {
            expect(&mut wire, Some("STARTTLS"), &[220], "STARTTLS")?;
            wire = wire.start_tls(&client)?;
            // What was said in the clear counts for nothing.
            extensions = expect(&mut wire, Some(&ehlo), &[250], "EHLO")?;
        } else if !wire.private() {
            return Err(fatal(format!(
                "{host} offers no STARTTLS: nothing is sent to it in the clear (use smtps://)"
            )));
        }
    }
    let offered = mechanisms(&extensions);
    // A relay on the machine itself may take mail without a login.
    if !offered.is_empty() || !is_loopback(&host) {
        smtp_login(&mut wire, config, &offered)?;
    }
    let international = !config.address.is_ascii() || !to.is_ascii();
    if international && !offers(&extensions, "SMTPUTF8") {
        return Err(fatal(format!(
            "{host} takes no addresses outside ASCII (SMTPUTF8)"
        )));
    }
    let utf8 = if international { " SMTPUTF8" } else { "" };
    expect(
        &mut wire,
        Some(&format!("MAIL FROM:<{}>{utf8}", config.address)),
        &[250],
        "MAIL FROM",
    )?;
    expect(
        &mut wire,
        Some(&format!("RCPT TO:<{to}>")),
        &[250, 251],
        &format!("RCPT TO {to}"),
    )?;
    expect(&mut wire, Some("DATA"), &[354], "DATA")?;
    // From here the server may have the message: it is not sent again.
    let delivered = wire
        .send(&dot_stuffed(message))
        .and_then(|()| expect(&mut wire, None, &[250], "the message"));
    if let Err(fault) = delivered {
        return Err(fatal(format!(
            "{} (the server may have the message: not sent again)",
            reason(fault)
        )));
    }
    let _ = expect(&mut wire, Some("QUIT"), &[221], "QUIT");
    Ok(())
}

/// `message` as DATA sends it: lines ending CRLF, a line starting with a
/// dot given another, and the closing dot.
pub fn dot_stuffed(message: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(message.len() + 64);
    let mut line_start = true;
    let mut i = 0;
    while i < message.len() {
        let b = message[i];
        if line_start && b == b'.' {
            out.push(b'.');
        }
        match b {
            b'\r' if message.get(i + 1) == Some(&b'\n') => {
                out.extend_from_slice(b"\r\n");
                i += 2;
                line_start = true;
                continue;
            }
            b'\r' | b'\n' => {
                out.extend_from_slice(b"\r\n");
                line_start = true;
            }
            _ => {
                out.push(b);
                line_start = false;
            }
        }
        i += 1;
    }
    if !out.is_empty() && !out.ends_with(b"\r\n") {
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(b".\r\n");
    out
}

// --- Messages read -------------------------------------------------------

/// A message, as read here.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Mail {
    /// The sender's address, lower-cased.
    pub from: String,
    /// The addresses it is written to.
    pub to: Vec<String>,
    /// The addresses it is copied to.
    pub cc: Vec<String>,
    /// Its subject.
    pub subject: String,
    /// Its Message-ID, without angle brackets.
    pub message_id: Option<String>,
    /// The messages it follows (`References`, then `In-Reply-To`).
    pub references: Vec<String>,
    /// What it says itself: the message it answers and the signature cut.
    pub body: String,
    /// What is attached and not read.
    pub attachments: Vec<String>,
    /// Its audio: media type, file name, bytes.
    pub audio: Vec<(String, String, Vec<u8>)>,
    /// Why it was written by a machine, if it was.
    pub automatic: Option<String>,
    /// Its `Authentication-Results`, topmost first.
    pub results: Vec<String>,
}

impl Mail {
    /// What it says, as the agent is given it: the subject first where it
    /// starts a conversation, and what is attached and not read.
    pub fn said(&self) -> String {
        let mut text = String::new();
        let subject = self.subject.trim();
        let reply = subject.len() >= 3 && subject[..3].eq_ignore_ascii_case("re:");
        if !subject.is_empty() && !reply && self.references.is_empty() {
            text.push_str(&format!("Subject: {subject}"));
        }
        if !self.body.is_empty() {
            if !text.is_empty() {
                text.push_str("\n\n");
            }
            text.push_str(&self.body);
        }
        for name in &self.attachments {
            text.push_str(&format!("\n(attached, not read: {name})"));
        }
        if text.chars().count() > MAX_TEXT {
            text = text.chars().take(MAX_TEXT).collect();
            text.push_str("\n(the rest is cut)");
        }
        text.trim().to_string()
    }
}

/// A message's header and body, split at the first empty line.
fn split_head(raw: &[u8]) -> (&[u8], &[u8]) {
    let mut pos = 0;
    while pos < raw.len() {
        let Some(end) = raw[pos..].iter().position(|b| *b == b'\n').map(|i| pos + i) else {
            return (raw, &[]);
        };
        let line = &raw[pos..end];
        if line.is_empty() || line == b"\r" {
            return (&raw[..pos], &raw[end + 1..]);
        }
        pos = end + 1;
    }
    (raw, &[])
}

/// The fields of a header, names lower-cased, folded lines joined.
fn fields(head: &[u8]) -> Vec<(String, String)> {
    let text = String::from_utf8_lossy(head);
    let mut out: Vec<(String, String)> = Vec::new();
    for line in text.split('\n') {
        let line = line.trim_end_matches('\r');
        if line.starts_with([' ', '\t']) {
            if let Some((_, value)) = out.last_mut() {
                value.push(' ');
                value.push_str(line.trim());
            }
        } else if let Some((name, value)) = line.split_once(':') {
            out.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
        }
    }
    out
}

fn field<'a>(fields: &'a [(String, String)], name: &str) -> Option<&'a str> {
    fields
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.as_str())
}

/// Text from a header field, encoded words (RFC 2047) decoded and no
/// control character left in it.
fn header_text(value: &str) -> String {
    decode_words(value)
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .trim()
        .to_string()
}

/// Decodes the encoded words (`=?utf-8?Q?…?=`) of a header's text; the
/// space between two of them goes.
pub fn decode_words(value: &str) -> String {
    let mut out = String::new();
    let mut rest = value;
    let mut after_word = false;
    while let Some(start) = rest.find("=?") {
        let before = &rest[..start];
        match encoded_word(&rest[start..]) {
            Some((decoded, len)) => {
                if !(after_word && before.chars().all(char::is_whitespace)) {
                    out.push_str(before);
                }
                out.push_str(&decoded);
                rest = &rest[start + len..];
                after_word = true;
            }
            None => {
                out.push_str(&rest[..start + 2]);
                rest = &rest[start + 2..];
                after_word = false;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The encoded word `text` starts with: what it says, and its length.
fn encoded_word(text: &str) -> Option<(String, usize)> {
    let inner = text.strip_prefix("=?")?;
    let (charset, inner) = inner.split_once('?')?;
    let (encoding, inner) = inner.split_once('?')?;
    let end = inner.find("?=")?;
    let data = &inner[..end];
    if data.contains(char::is_whitespace) || charset.is_empty() {
        return None;
    }
    let bytes = match encoding {
        "B" | "b" => base64_lenient(data.as_bytes()),
        "Q" | "q" => q_decode(data),
        _ => return None,
    };
    // RFC 2231's language comes after a star.
    let charset = charset.split('*').next().unwrap_or(charset);
    let len = text.len() - inner.len() + end + 2;
    Some((decode_charset(&bytes, charset), len))
}

fn hex_value(b: u8) -> Option<u8> {
    (b as char).to_digit(16).map(|d| d as u8)
}

fn q_decode(data: &str) -> Vec<u8> {
    let bytes = data.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'_' => out.push(b' '),
            b'=' => {
                match (
                    bytes.get(i + 1).copied().and_then(hex_value),
                    bytes.get(i + 2).copied().and_then(hex_value),
                ) {
                    (Some(h), Some(l)) => {
                        out.push(h * 16 + l);
                        i += 3;
                        continue;
                    }
                    _ => out.push(b'='),
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    out
}

/// Base64 as mail writes it: across lines, padded or not, and blocks
/// padded one after another.
fn base64_lenient(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() * 3 / 4);
    let mut block = Vec::new();
    let flush = |block: &mut Vec<u8>, out: &mut Vec<u8>| {
        // A last character on its own carries no byte.
        if block.len() % 4 == 1 {
            block.pop();
        }
        if let Ok(bytes) = LENIENT.decode(block.as_slice()) {
            out.extend_from_slice(&bytes);
        }
        block.clear();
    };
    for &b in data {
        if b.is_ascii_alphanumeric() || b == b'+' || b == b'/' {
            block.push(b);
        } else if b == b'=' && !block.is_empty() {
            flush(&mut block, &mut out);
        }
    }
    flush(&mut block, &mut out);
    out
}

fn quoted_printable(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        if data[i] == b'=' {
            match (data.get(i + 1), data.get(i + 2)) {
                // A soft line break.
                (Some(b'\r'), Some(b'\n')) => {
                    i += 3;
                    continue;
                }
                (Some(b'\n'), _) => {
                    i += 2;
                    continue;
                }
                (Some(&h), Some(&l)) => {
                    if let (Some(h), Some(l)) = (hex_value(h), hex_value(l)) {
                        out.push(h * 16 + l);
                        i += 3;
                        continue;
                    }
                }
                _ => {}
            }
        }
        out.push(data[i]);
        i += 1;
    }
    out
}

/// Windows-1252's characters for 0x80-0x9F.
const CP1252: [u16; 32] = [
    0x20AC, 0xFFFD, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039,
    0x0152, 0xFFFD, 0x017D, 0xFFFD, 0xFFFD, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0xFFFD, 0x017E, 0x0178,
];

/// Text in `charset`: UTF-8 and ASCII, Latin-1 read as Windows-1252 (as
/// browsers do), Windows-1252 and Latin-9; anything else as UTF-8, where
/// it agrees.
pub fn decode_charset(bytes: &[u8], charset: &str) -> String {
    let charset = charset.trim().trim_matches('"').to_ascii_lowercase();
    match charset.as_str() {
        "iso-8859-1" | "iso8859-1" | "iso_8859-1" | "latin1" | "latin-1" | "l1"
        | "windows-1252" | "cp1252" | "x-cp1252" => bytes
            .iter()
            .map(|&b| match b {
                0x80..=0x9F => {
                    char::from_u32(u32::from(CP1252[usize::from(b - 0x80)])).unwrap_or('\u{FFFD}')
                }
                _ => char::from(b),
            })
            .collect(),
        "iso-8859-15" | "iso8859-15" | "latin-9" | "latin9" => bytes
            .iter()
            .map(|&b| match b {
                0xA4 => '€',
                0xA6 => 'Š',
                0xA8 => 'š',
                0xB4 => 'Ž',
                0xB8 => 'ž',
                0xBC => 'Œ',
                0xBD => 'œ',
                0xBE => 'Ÿ',
                _ => char::from(b),
            })
            .collect(),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// The addresses of an address field (`From`, `To`), lower-cased.
pub fn addresses(value: &str) -> Vec<String> {
    fn push(out: &mut Vec<String>, item: &str) {
        let item = item.trim();
        let address = match item.find('<') {
            Some(start) => item[start + 1..].split('>').next().unwrap_or_default(),
            None => item,
        };
        let address = address.trim().to_lowercase();
        if valid_address(&address) {
            out.push(address);
        }
    }
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut comment = 0u32;
    let mut angle = false;
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' if quoted || comment > 0 => {
                chars.next();
            }
            '(' if !quoted => comment += 1,
            ')' if !quoted && comment > 0 => comment -= 1,
            _ if comment > 0 => {}
            '"' => quoted = !quoted,
            _ if quoted => {}
            ',' | ';' if !angle => {
                push(&mut out, &current);
                current.clear();
            }
            // A group's name.
            ':' if !angle => current.clear(),
            '<' => {
                angle = true;
                current.clear();
                current.push('<');
            }
            '>' if angle => {
                angle = false;
                current.push('>');
            }
            _ => current.push(c),
        }
    }
    push(&mut out, &current);
    out
}

/// Whether `address` can be written to as it is: one `@` between a local
/// part and a domain, and nothing a command or a header would take apart.
pub fn valid_address(address: &str) -> bool {
    let Some((local, domain)) = address.rsplit_once('@') else {
        return false;
    };
    !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && address.len() <= 254
        && !address
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "<>,;:\"()[]\\".contains(c))
        && !domain.contains('@')
}

fn domain_of(address: &str) -> &str {
    address.rsplit_once('@').map_or(address, |(_, d)| d)
}

/// The Message-IDs a field names, without angle brackets.
fn message_ids(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = value;
    while let Some(start) = rest.find('<') {
        let Some(end) = rest[start..].find('>') else {
            break;
        };
        let id = &rest[start + 1..start + end];
        if !id.is_empty() && !id.contains(char::is_whitespace) && id.len() <= 250 {
            out.push(id.to_string());
        }
        rest = &rest[start + end + 1..];
    }
    out
}

/// A field's parts after its value (`; charset=utf-8`), split outside
/// quotes.
fn split_params(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for c in value.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                current.push(c);
            }
            ';' if !quoted => out.push(std::mem::take(&mut current)),
            _ => current.push(c),
        }
    }
    out.push(current);
    out
}

fn percent_decoded(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if let (Some(h), Some(l)) = (
                bytes.get(i + 1).copied().and_then(hex_value),
                bytes.get(i + 2).copied().and_then(hex_value),
            ) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

/// A field's value lower-cased (`text/plain`) and its parameters.
fn value_and_params(value: &str) -> (String, BTreeMap<String, String>) {
    let parts = split_params(value);
    let main = parts
        .first()
        .map(|m| m.trim().to_ascii_lowercase())
        .unwrap_or_default();
    let mut params = BTreeMap::new();
    for part in parts.iter().skip(1) {
        let Some((key, value)) = part.split_once('=') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .map(|v| v.replace("\\\"", "\"").replace("\\\\", "\\"))
            .unwrap_or_else(|| value.to_string());
        match key.strip_suffix('*') {
            // RFC 2231: charset'language'percent-encoded.
            Some(key) => {
                let mut pieces = value.splitn(3, '\'');
                let (charset, _, data) = (pieces.next(), pieces.next(), pieces.next());
                let decoded = match data {
                    Some(data) => decode_charset(&percent_decoded(data), charset.unwrap_or("")),
                    None => String::from_utf8_lossy(&percent_decoded(&value)).into_owned(),
                };
                params.insert(key.to_string(), decoded);
            }
            None => {
                params.entry(key).or_insert_with(|| header_text(&value));
            }
        }
    }
    (main, params)
}

/// What the parts of a message hold.
#[derive(Default)]
struct Parts {
    plain: Vec<String>,
    html: Vec<String>,
    attachments: Vec<String>,
    audio: Vec<(String, String, Vec<u8>)>,
}

impl Parts {
    fn take(&mut self, other: Parts) {
        self.plain.extend(other.plain);
        self.html.extend(other.html);
        self.attachments.extend(other.attachments);
        self.audio.extend(other.audio);
    }
}

/// A file name for audio of `media_type`, unless it has a name of its own.
fn audio_name(media_type: &str, name: Option<&str>) -> String {
    if let Some(name) = name.filter(|n| n.contains('.')) {
        return name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || ".-_".contains(c) {
                    c
                } else {
                    '_'
                }
            })
            .collect();
    }
    let extension = match media_type {
        "audio/ogg" | "audio/opus" => "ogg",
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/mp4" | "audio/m4a" | "audio/x-m4a" | "audio/aac" => "m4a",
        "audio/wav" | "audio/x-wav" | "audio/wave" => "wav",
        "audio/webm" => "webm",
        "audio/flac" | "audio/x-flac" => "flac",
        _ => "audio",
    };
    format!("voice.{extension}")
}

/// Walks a part (a whole message's body at first) for its text, audio
/// and attachments.
fn walk(fields: &[(String, String)], body: &[u8], depth: usize, parts: &mut Parts) {
    let (media, params) = value_and_params(field(fields, "content-type").unwrap_or("text/plain"));
    let media = if media.contains('/') {
        media
    } else {
        "text/plain".to_string()
    };
    let (disposition, disposition_params) =
        value_and_params(field(fields, "content-disposition").unwrap_or(""));
    let attachment = disposition == "attachment";
    let name = disposition_params
        .get("filename")
        .or_else(|| params.get("name"))
        .cloned();
    if media.starts_with("multipart/") && depth < 10 {
        let Some(boundary) = params.get("boundary") else {
            return;
        };
        let sections = split_multipart(body, boundary);
        if media == "multipart/alternative" {
            // The plainest alternative that has text.
            let mut best: Option<Parts> = None;
            for section in sections {
                let (head, body) = split_head(section);
                let mut alternative = Parts::default();
                walk(&fields_of(head), body, depth + 1, &mut alternative);
                let better = match &best {
                    None => true,
                    Some(best) => {
                        best.plain.is_empty()
                            && (!alternative.plain.is_empty()
                                || (best.html.is_empty() && !alternative.html.is_empty()))
                    }
                };
                if better {
                    best = Some(alternative);
                }
            }
            if let Some(best) = best {
                parts.take(best);
            }
        } else {
            for section in sections {
                let (head, body) = split_head(section);
                walk(&fields_of(head), body, depth + 1, parts);
            }
        }
        return;
    }
    let decoded = match field(fields, "content-transfer-encoding")
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "base64" => base64_lenient(body),
        "quoted-printable" => quoted_printable(body),
        _ => body.to_vec(),
    };
    let charset = params.get("charset").map(String::as_str).unwrap_or("utf-8");
    if media.starts_with("audio/") {
        if parts.audio.len() < 4 {
            let name = audio_name(&media, name.as_deref());
            parts.audio.push((media, name, decoded));
        }
        return;
    }
    if media == "message/rfc822" && depth < 10 {
        // A message forwarded as itself: read as part of this one.
        let inner = parse_at(&decoded, depth + 1);
        let from = if inner.from.is_empty() {
            "someone".to_string()
        } else {
            inner.from.clone()
        };
        parts.plain.push(format!(
            "(a message forwarded with it, from {from}: {})\n{}",
            inner.subject, inner.body
        ));
        return;
    }
    if !attachment && media == "text/plain" {
        let text = decode_charset(&decoded, charset);
        let flowed = params
            .get("format")
            .is_some_and(|f| f.eq_ignore_ascii_case("flowed"));
        let delsp = params
            .get("delsp")
            .is_some_and(|d| d.eq_ignore_ascii_case("yes"));
        parts
            .plain
            .push(if flowed { unflow(&text, delsp) } else { text });
        return;
    }
    if !attachment && media == "text/html" {
        parts
            .html
            .push(html_text(&decode_charset(&decoded, charset)));
        return;
    }
    if attachment || name.is_some() {
        parts.attachments.push(format!(
            "{}, {media}, {} bytes",
            name.unwrap_or_else(|| "a file".into()),
            decoded.len()
        ));
    }
}

fn fields_of(head: &[u8]) -> Vec<(String, String)> {
    fields(head)
}

/// The parts of a multipart body, between its boundaries.
fn split_multipart<'a>(body: &'a [u8], boundary: &str) -> Vec<&'a [u8]> {
    let delimiter = format!("--{boundary}");
    let mut sections = Vec::new();
    let mut start: Option<usize> = None;
    let mut pos = 0;
    let mut closed = false;
    while pos < body.len() {
        let end = body[pos..]
            .iter()
            .position(|b| *b == b'\n')
            .map_or(body.len(), |i| pos + i);
        let mut line = &body[pos..end];
        if line.last() == Some(&b'\r') {
            line = &line[..line.len() - 1];
        }
        if let Some(rest) = line.strip_prefix(delimiter.as_bytes()) {
            let closing = rest.starts_with(b"--");
            if closing || rest.iter().all(u8::is_ascii_whitespace) {
                if let Some(s) = start {
                    // The line break before a boundary belongs to it.
                    let mut e = pos;
                    if e > s && body[e - 1] == b'\n' {
                        e -= 1;
                        if e > s && body[e - 1] == b'\r' {
                            e -= 1;
                        }
                    }
                    sections.push(&body[s..e]);
                }
                if closing {
                    closed = true;
                    break;
                }
                start = Some((end + 1).min(body.len()));
            }
        }
        pos = end + 1;
    }
    if !closed {
        if let Some(s) = start.filter(|s| *s < body.len()) {
            sections.push(&body[s..]);
        }
    }
    sections
}

/// Text sent as format=flowed (RFC 3676), its soft line breaks joined.
fn unflow(text: &str, delsp: bool) -> String {
    let mut out = String::new();
    for line in text.split('\n') {
        let line = line.trim_end_matches('\r');
        let line = line.strip_prefix(' ').unwrap_or(line);
        if line.ends_with(' ') && line != "-- " {
            out.push_str(if delsp { &line[..line.len() - 1] } else { line });
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// The text of an HTML message: its words, blocks on lines of their own,
/// quoted messages (blockquotes, Gmail's, Outlook's and others' quotes)
/// left out.
pub fn html_text(html: &str) -> String {
    let mut out = String::new();
    let mut skipping: Option<(String, u32)> = None;
    let mut rest = html;
    while !rest.is_empty() {
        let Some(open) = rest.find('<') else {
            if skipping.is_none() {
                words(&mut out, rest);
            }
            break;
        };
        if skipping.is_none() {
            words(&mut out, &rest[..open]);
        }
        rest = &rest[open..];
        if let Some(comment) = rest.strip_prefix("<!--") {
            rest = comment.find("-->").map_or("", |e| &comment[e + 3..]);
            continue;
        }
        let Some(close) = tag_end(rest) else {
            break;
        };
        let tag = &rest[1..close];
        rest = &rest[close + 1..];
        let closing = tag.starts_with('/');
        let name = tag
            .trim_start_matches('/')
            .chars()
            .take_while(char::is_ascii_alphanumeric)
            .collect::<String>()
            .to_ascii_lowercase();
        if let Some((skipped, depth)) = &mut skipping {
            if name == *skipped {
                if closing {
                    *depth -= 1;
                    if *depth == 0 {
                        skipping = None;
                    }
                } else if !tag.ends_with('/') {
                    *depth += 1;
                }
            }
            continue;
        }
        let quote = name == "blockquote" || (name == "div" && quoting(tag));
        if !closing && (quote || matches!(name.as_str(), "script" | "style" | "head" | "title")) {
            skipping = Some((name, 1));
            continue;
        }
        match name.as_str() {
            "br" | "p" | "div" | "tr" | "table" | "ul" | "ol" | "hr" | "pre" | "section"
            | "article" | "header" | "footer" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                out.push('\n')
            }
            "li" if !closing => out.push_str("\n- "),
            "td" | "th" if closing => out.push(' '),
            _ => {}
        }
    }
    let lines: Vec<String> = out.lines().map(|l| l.trim().to_string()).collect();
    tidy(&lines.join("\n"))
}

/// Whether a `div` holds a quoted message (by the classes and ids mail
/// programs give them).
fn quoting(tag: &str) -> bool {
    let tag = tag.to_ascii_lowercase();
    [
        "gmail_quote",
        "divrplyfwdmsg",
        "appendonsend",
        "yahoo_quoted",
        "protonmail_quote",
        "moz-cite-prefix",
    ]
    .iter()
    .any(|marker| tag.contains(marker))
}

/// Where a tag that `text` starts with ends (its `>`, outside quotes).
fn tag_end(text: &str) -> Option<usize> {
    let mut quote: Option<char> = None;
    for (i, c) in text.char_indices().skip(1) {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), _) if c == q => quote = None,
            (None, '>') => return Some(i),
            _ => {}
        }
    }
    None
}

/// Appends HTML text, entities decoded and spaces collapsed.
fn words(out: &mut String, text: &str) {
    let decoded = entities(text);
    let mut space = out.ends_with([' ', '\n']) || out.is_empty();
    for c in decoded.chars() {
        if c.is_whitespace() {
            if !space {
                out.push(' ');
                space = true;
            }
        } else {
            out.push(c);
            space = false;
        }
    }
}

fn entities(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let end = rest
            .char_indices()
            .take(12)
            .find(|(_, c)| *c == ';')
            .map(|(i, _)| i);
        if let Some(end) = end {
            let name = &rest[1..end];
            let decoded = match name {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some(' '),
                "mdash" => Some('—'),
                "ndash" => Some('–'),
                "hellip" => Some('…'),
                "lsquo" => Some('‘'),
                "rsquo" => Some('’'),
                "ldquo" => Some('“'),
                "rdquo" => Some('”'),
                "euro" => Some('€'),
                "copy" => Some('©'),
                "reg" => Some('®'),
                "trade" => Some('™'),
                "bull" => Some('•'),
                "middot" => Some('·'),
                n if n.starts_with("#x") || n.starts_with("#X") => u32::from_str_radix(&n[2..], 16)
                    .ok()
                    .and_then(char::from_u32),
                n if n.starts_with('#') => n[1..].parse().ok().and_then(char::from_u32),
                _ => None,
            };
            if let Some(c) = decoded {
                out.push(c);
                rest = &rest[end + 1..];
                continue;
            }
        }
        out.push('&');
        rest = &rest[1..];
    }
    out.push_str(rest);
    out
}

/// Lines without trailing spaces, at most one empty line in a row, none
/// at either end.
fn tidy(text: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut empty = 0;
    for line in text.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            empty += 1;
            if empty > 1 {
                continue;
            }
        } else {
            empty = 0;
        }
        out.push(line);
    }
    out.join("\n").trim_matches('\n').to_string()
}

/// How replies are introduced (`On Mon, … Alice wrote:`), in the languages
/// mail programs write it in: how such a line starts, and how it ends.
const ATTRIBUTION_STARTS: [&str; 9] = [
    "On ", "Le ", "Am ", "El ", "Il ", "Em ", "Op ", "Den ", "W dniu ",
];
const ATTRIBUTION_ENDS: [&str; 10] = [
    "wrote:",
    "a écrit :",
    "a écrit:",
    "schrieb:",
    "escribió:",
    "ha scritto:",
    "escreveu:",
    "schreef:",
    "skrev:",
    "napisał:",
];

fn ends_attribution(line: &str) -> bool {
    ATTRIBUTION_ENDS
        .iter()
        .any(|e| line.trim_end().ends_with(e))
}

fn starts_attribution(line: &str) -> bool {
    ATTRIBUTION_STARTS.iter().any(|s| line.starts_with(s))
}

/// What a message says itself: the message it answers (after its
/// attribution, or quoted with `>`) and the signature (after `-- `) cut.
pub fn own_words(text: &str) -> String {
    let text = text.replace("\r\n", "\n");
    let lines: Vec<&str> = text.split('\n').collect();
    // Whether only quoted or empty lines follow line `i`.
    let quoted_after = |i: usize| {
        lines[i + 1..]
            .iter()
            .find(|l| !l.trim().is_empty())
            .map_or(true, |l| l.trim_start().starts_with('>'))
    };
    let mut kept: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();
        if line == "-- " || trimmed == "--" {
            break;
        }
        let attribution = starts_attribution(trimmed)
            && (ends_attribution(trimmed)
                || lines
                    .get(i + 1)
                    .is_some_and(|next| next.trim().len() < 200 && ends_attribution(next.trim())));
        if attribution {
            let last = if ends_attribution(trimmed) { i } else { i + 1 };
            if quoted_after(last) {
                break;
            }
        }
        let outlook = trimmed.len() >= 20
            && trimmed.chars().all(|c| c == '_')
            && lines[i + 1..]
                .iter()
                .find(|l| !l.trim().is_empty())
                .is_some_and(|l| l.trim_start().starts_with("From:"));
        if outlook
            || trimmed.starts_with("-----Original Message-----")
            || trimmed.starts_with("-------- Original Message --------")
        {
            break;
        }
        if !trimmed.starts_with('>') {
            kept.push(line);
        }
        i += 1;
    }
    tidy(&kept.join("\n"))
}

/// Why a message was written by a machine (RFC 3834's signs and others),
/// if it was.
fn automatic(fields: &[(String, String)], from: &str, media: &str) -> Option<String> {
    if let Some(value) = field(fields, "auto-submitted") {
        let value = value.split(';').next().unwrap_or_default().trim();
        if !value.eq_ignore_ascii_case("no") {
            return Some(format!("an automatic message (Auto-Submitted: {value})"));
        }
    }
    if let Some(value) = field(fields, "precedence") {
        let value = value.trim().to_ascii_lowercase();
        if matches!(value.as_str(), "bulk" | "junk" | "list") {
            return Some(format!("a message marked Precedence: {value}"));
        }
    }
    if field(fields, "list-id").is_some() || field(fields, "list-unsubscribe").is_some() {
        return Some("a mailing list's message".into());
    }
    if field(fields, "x-autoreply").is_some() || field(fields, "x-autorespond").is_some() {
        return Some("an automatic reply".into());
    }
    if media == "multipart/report" {
        return Some("a delivery report".into());
    }
    if field(fields, "return-path").is_some_and(|r| r.trim() == "<>") {
        return Some("a message with no return path (a bounce)".into());
    }
    let local = from.split('@').next().unwrap_or_default();
    if local.eq_ignore_ascii_case("mailer-daemon") || local.eq_ignore_ascii_case("postmaster") {
        return Some("a mail server's message".into());
    }
    None
}

/// Reads a message.
pub fn parse(raw: &[u8]) -> Mail {
    parse_at(raw, 0)
}

fn parse_at(raw: &[u8], depth: usize) -> Mail {
    let (head, body) = split_head(raw);
    let fields = fields(head);
    let from = field(&fields, "from")
        .map(addresses)
        .and_then(|a| a.into_iter().next())
        .unwrap_or_default();
    let list = |name: &str| {
        fields
            .iter()
            .filter(|(n, _)| n == name)
            .flat_map(|(_, v)| addresses(v))
            .collect::<Vec<_>>()
    };
    let message_id = field(&fields, "message-id").and_then(|v| message_ids(v).into_iter().next());
    let mut references: Vec<String> = field(&fields, "references")
        .map(message_ids)
        .unwrap_or_default();
    for id in field(&fields, "in-reply-to")
        .map(message_ids)
        .unwrap_or_default()
    {
        if !references.contains(&id) {
            references.push(id);
        }
    }
    let (media, _) = value_and_params(field(&fields, "content-type").unwrap_or("text/plain"));
    let mut parts = Parts::default();
    walk(&fields, body, depth, &mut parts);
    let text = if parts.plain.iter().any(|t| !t.trim().is_empty()) {
        parts.plain.join("\n\n")
    } else {
        parts.html.join("\n\n")
    };
    Mail {
        automatic: automatic(&fields, &from, &media),
        to: list("to"),
        cc: list("cc"),
        subject: field(&fields, "subject")
            .map(header_text)
            .unwrap_or_default(),
        message_id,
        references,
        body: own_words(&text),
        attachments: parts.attachments,
        audio: parts.audio,
        results: fields
            .iter()
            .filter(|(n, _)| n == "authentication-results")
            .map(|(_, v)| v.clone())
            .collect(),
        from,
    }
}

// --- Who wrote it --------------------------------------------------------

/// Text without its comments (`(…)`, nested, outside quotes).
fn without_comments(text: &str) -> String {
    let mut out = String::new();
    let mut depth = 0u32;
    let mut quoted = false;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' if quoted || depth > 0 => {
                if let Some(next) = chars.next() {
                    if depth == 0 {
                        out.push(c);
                        out.push(next);
                    }
                }
            }
            '"' if depth == 0 => {
                quoted = !quoted;
                out.push(c);
            }
            '(' if !quoted => depth += 1,
            ')' if !quoted && depth > 0 => depth -= 1,
            _ if depth > 0 => {}
            _ => out.push(c),
        }
    }
    out
}

/// One method's result in `Authentication-Results`: `dkim=pass
/// header.d=example.org`.
#[derive(Debug, PartialEq, Eq)]
struct Verdict {
    method: String,
    result: String,
    properties: BTreeMap<String, String>,
}

/// The server that wrote results (their authserv-id), and the results.
fn verdicts(results: &str) -> (Option<String>, Vec<Verdict>) {
    let text = without_comments(results);
    let mut server = None;
    let mut out = Vec::new();
    let mut first = true;
    for statement in text.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        let mut words = statement.split_whitespace();
        let Some(head) = words.next() else {
            continue;
        };
        if first {
            first = false;
            // Microsoft's results name no server: they start with one.
            if !head.contains('=') {
                server = Some(head.to_ascii_lowercase());
                continue;
            }
        }
        let Some((method, result)) = head.split_once('=') else {
            continue;
        };
        let properties = words
            .filter_map(|w| w.split_once('='))
            .map(|(k, v)| {
                (
                    k.to_ascii_lowercase(),
                    v.trim_matches('"').to_ascii_lowercase(),
                )
            })
            .collect();
        out.push(Verdict {
            method: method
                .split('/')
                .next()
                .unwrap_or(method)
                .to_ascii_lowercase(),
            result: result.to_ascii_lowercase(),
            properties,
        });
    }
    (server, out)
}

/// Whether two domains belong together (DMARC's relaxed alignment, taken
/// as one being the other or under it).
fn aligned(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim_end_matches('.'), b.trim_end_matches('.'));
    if a.is_empty() || b.is_empty() {
        return false;
    }
    let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    short.eq_ignore_ascii_case(long)
        || (short.contains('.')
            && long
                .to_ascii_lowercase()
                .ends_with(&format!(".{}", short.to_ascii_lowercase())))
}

/// Whether `results` (a message's `Authentication-Results`, topmost
/// first) show it to be from `from`: the topmost (or with `authserv`, the
/// topmost that server wrote) has DMARC passing for `from`'s domain, or
/// DKIM or SPF passing for a domain aligned with it, and DMARC failing
/// for none.
pub fn authenticated(results: &[String], from: &str, authserv: Option<&str>) -> Result<(), String> {
    let domain = domain_of(from).to_ascii_lowercase();
    let chosen = results
        .iter()
        .map(|r| verdicts(r))
        .find(|(server, _)| match authserv {
            None => true,
            Some(id) => server
                .as_deref()
                .is_some_and(|s| aligned(s, id) && s.len() >= id.len()),
        });
    let Some((_, verdicts)) = chosen else {
        return Err(match authserv {
            Some(id) => format!("no Authentication-Results from {id}"),
            None => "no Authentication-Results: the mail server checked nothing (EMAIL_TRUST_FROM=1 takes From as it is)".into(),
        });
    };
    let failed = verdicts.iter().find(|v| {
        v.method == "dmarc"
            && matches!(
                v.result.as_str(),
                "fail" | "permerror" | "temperror" | "quarantine" | "reject"
            )
    });
    if let Some(failed) = failed {
        return Err(format!("DMARC {} for {domain}", failed.result));
    }
    let property_domain = |v: &Verdict, names: &[&str]| -> Option<String> {
        names
            .iter()
            .find_map(|n| v.properties.get(*n))
            .map(|value| domain_of(value).to_string())
    };
    let passed = verdicts.iter().any(|v| {
        v.result == "pass"
            && match v.method.as_str() {
                "dmarc" => property_domain(v, &["header.from"]).map_or(true, |d| d == domain),
                "dkim" => property_domain(v, &["header.d", "header.i"])
                    .is_some_and(|d| aligned(&d, &domain)),
                "spf" => {
                    property_domain(v, &["smtp.mailfrom"]).is_some_and(|d| aligned(&d, &domain))
                }
                _ => false,
            }
    });
    if passed {
        return Ok(());
    }
    let summary = verdicts
        .iter()
        .filter(|v| matches!(v.method.as_str(), "dmarc" | "dkim" | "spf"))
        .map(|v| format!("{}={}", v.method, v.result))
        .collect::<Vec<_>>()
        .join(", ");
    Err(format!(
        "not shown to be from {domain} ({})",
        if summary.is_empty() {
            "no DMARC, DKIM or SPF result"
        } else {
            summary.as_str()
        }
    ))
}

// --- Messages sent -------------------------------------------------------

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    if sion_http::random(&mut buf).is_err() {
        // Uniqueness, not secrecy: the time and a counter do.
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        return format!("{:x}{n:x}", now());
    }
    hex::encode(buf)
}

/// A time as a message's `Date` (RFC 5322), in UTC.
pub fn date(t: u64) -> String {
    let days = t / 86_400;
    let (year, month, day) = sion_agent::schedule::civil(days);
    // 1970-01-01 was a Thursday.
    let weekday = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"][(days % 7) as usize];
    let month = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ][(month as usize).saturating_sub(1).min(11)];
    let s = t % 86_400;
    format!(
        "{weekday}, {day} {month} {year} {:02}:{:02}:{:02} +0000",
        s / 3600,
        s / 60 % 60,
        s % 60
    )
}

/// Header text as it may be sent: as it is in printable ASCII, in encoded
/// words (UTF-8, Base64) otherwise, each short enough for a line.
pub fn encode_header(text: &str) -> String {
    let text: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if text.bytes().all(|b| (b' '..=b'~').contains(&b)) && !text.contains("=?") {
        return text;
    }
    let mut words = Vec::new();
    let mut chunk = String::new();
    for c in text.chars() {
        if chunk.len() + c.len_utf8() > 45 {
            words.push(format!("=?utf-8?B?{}?=", STANDARD.encode(chunk.as_bytes())));
            chunk.clear();
        }
        chunk.push(c);
    }
    if !chunk.is_empty() {
        words.push(format!("=?utf-8?B?{}?=", STANDARD.encode(chunk.as_bytes())));
    }
    words.join("\r\n ")
}

/// A display name as `From` takes it.
fn display_name(name: &str) -> String {
    let plain = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || " !#$%&'*+-/=?^_`{|}~".contains(c));
    if plain {
        name.trim().to_string()
    } else if name.bytes().all(|b| (b' '..=b'~').contains(&b)) {
        format!("\"{}\"", name.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        encode_header(name)
    }
}

/// Text as quoted-printable, in lines of at most 76 characters.
pub fn quoted_printable_encoded(text: &str) -> String {
    let mut out = String::new();
    for (n, line) in text.replace("\r\n", "\n").split('\n').enumerate() {
        if n > 0 {
            out.push_str("\r\n");
        }
        let bytes = line.as_bytes();
        let mut width = 0;
        for (i, &b) in bytes.iter().enumerate() {
            let last = i + 1 == bytes.len();
            let piece =
                if ((b == b' ' || b == b'\t') && !last) || ((33..=126).contains(&b) && b != b'=') {
                    (b as char).to_string()
                } else {
                    format!("={b:02X}")
                };
            if width + piece.len() > 75 {
                out.push_str("=\r\n");
                width = 0;
            }
            out.push_str(&piece);
            width += piece.len();
        }
    }
    out
}

/// The subject of a reply to `subject`.
fn reply_subject(subject: &str) -> String {
    let subject = subject.trim();
    if subject.is_empty() {
        "Re: your message".into()
    } else if subject.len() >= 3 && subject[..3].eq_ignore_ascii_case("re:") {
        subject.to_string()
    } else {
        format!("Re: {subject}")
    }
}

/// A message from `address` (as `name`) to `to`, in `thread`, saying
/// `text`, with `audio` attached, sent at `t` as `<id@domain>`.
#[allow(clippy::too_many_arguments)]
fn compose(
    address: &str,
    name: Option<&str>,
    to: &str,
    thread: Option<&Thread>,
    text: &str,
    audio: Option<&Audio>,
    t: u64,
    id: &str,
) -> Vec<u8> {
    let mut head = String::new();
    let mut line = |name: &str, value: &str| head.push_str(&format!("{name}: {value}\r\n"));
    line(
        "From",
        &match name {
            Some(name) => format!("{} <{address}>", display_name(name)),
            None => format!("<{address}>"),
        },
    );
    line("To", &format!("<{to}>"));
    line(
        "Subject",
        &encode_header(&reply_subject(
            thread.map(|t| t.subject.as_str()).unwrap_or_default(),
        )),
    );
    line("Date", &date(t));
    line("Message-ID", &format!("<{id}@{}>", domain_of(address)));
    if let Some(thread) = thread {
        if let Some(parent) = &thread.message_id {
            line("In-Reply-To", &format!("<{parent}>"));
        }
        if !thread.references.is_empty() {
            let references: Vec<String> =
                thread.references.iter().map(|r| format!("<{r}>")).collect();
            line("References", &references.join("\r\n "));
        }
    }
    // Written by a machine: not to be answered by one (RFC 3834).
    line("Auto-Submitted", "auto-replied");
    line("MIME-Version", "1.0");
    let text_part = format!(
        "Content-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\n{}\r\n",
        quoted_printable_encoded(text)
    );
    match audio {
        None => head.push_str(&text_part),
        Some(audio) => {
            let boundary = format!("sion-{id}");
            let encoded = STANDARD.encode(audio.bytes.as_slice());
            let lines: Vec<&str> = encoded
                .as_bytes()
                .chunks(76)
                .map(|c| std::str::from_utf8(c).unwrap_or_default())
                .collect();
            let file = audio_name(&audio.media_type, Some(&audio.name));
            head.push_str(&format!(
                "Content-Type: multipart/mixed; boundary=\"{boundary}\"\r\n\r\n--{boundary}\r\n{text_part}--{boundary}\r\nContent-Type: {}\r\nContent-Disposition: attachment; filename=\"{file}\"\r\nContent-Transfer-Encoding: base64\r\n\r\n{}\r\n--{boundary}--\r\n",
                audio.media_type.replace(['\r', '\n'], ""),
                lines.join("\r\n")
            ));
        }
    }
    head.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    const GMAIL_RESULTS: &str = "mx.google.com;\r\n       dkim=pass header.i=@example.org header.s=s1 header.b=abc;\r\n       spf=pass (google.com: domain of alice@example.org designates 192.0.2.1 as permitted sender) smtp.mailfrom=alice@example.org;\r\n       dmarc=pass (p=REJECT sp=REJECT dis=NONE) header.from=example.org";

    fn message(lines: &[&str]) -> Vec<u8> {
        lines.join("\r\n").into_bytes()
    }

    #[test]
    fn a_reply_is_read_for_its_own_words() {
        let raw = message(&[
            "Return-Path: <alice@example.org>",
            &format!("Authentication-Results: {GMAIL_RESULTS}"),
            "From: \"Example, Alice\" <Alice@Example.org>",
            "To: Lead <lead@example.net>, bob@example.org",
            "Cc: (the team) carol@example.org",
            "Subject: =?utf-8?Q?Re:_Caf=C3=A9?= =?utf-8?B?IGJ1ZGdldA==?=",
            "Message-ID: <m4@example.org>",
            "In-Reply-To: <r1@example.net>",
            "References: <m1@example.org>\r\n <r1@example.net>",
            "MIME-Version: 1.0",
            "Content-Type: multipart/alternative; boundary=\"b1\"",
            "",
            "--b1",
            "Content-Type: text/plain; charset=\"UTF-8\"",
            "Content-Transfer-Encoding: quoted-printable",
            "",
            "How much budget is left for the caf=C3=A9? It's a long line that goes on=",
            " and on.",
            "",
            "On Mon, Oct 26, 2026 at 10:01 AM Lead <lead@example.net>",
            "wrote:",
            "> 100 USD.",
            ">",
            "--b1",
            "Content-Type: text/html; charset=\"UTF-8\"",
            "",
            "<div>How much budget is left?</div>",
            "--b1--",
            "",
        ]);
        let mail = parse(&raw);
        assert_eq!(mail.from, "alice@example.org");
        assert_eq!(mail.to, ["lead@example.net", "bob@example.org"]);
        assert_eq!(mail.cc, ["carol@example.org"]);
        assert_eq!(mail.subject, "Re: Café budget");
        assert_eq!(mail.message_id.as_deref(), Some("m4@example.org"));
        assert_eq!(mail.references, ["m1@example.org", "r1@example.net"]);
        assert_eq!(
            mail.body,
            "How much budget is left for the café? It's a long line that goes on and on."
        );
        assert_eq!(mail.automatic, None);
        assert_eq!(mail.results.len(), 1);
        // A reply: its subject is the conversation's, not said again.
        assert_eq!(mail.said(), mail.body);
        assert_eq!(authenticated(&mail.results, &mail.from, None), Ok(()));
    }

    #[test]
    fn html_charsets_and_attachments_are_read() {
        let raw = message(&[
            "From: bob@example.org",
            "To: lead@example.net",
            "Subject: Invoice",
            "Content-Type: multipart/mixed; boundary=outer",
            "",
            "preamble",
            "--outer",
            "Content-Type: text/html; charset=iso-8859-1",
            "Content-Transfer-Encoding: quoted-printable",
            "",
            "<html><head><style>p{}</style></head><body><p>Caf=E9 &amp; =93tea=94</p>",
            "<ul><li>one</li><li>two</li></ul><div class=3D\"gmail_quote\"><div>old</div> text</div>",
            "<blockquote>quoted</blockquote>after</body></html>",
            "--outer",
            "Content-Type: application/pdf; name=\"invoice.pdf\"",
            "Content-Disposition: attachment; filename*=utf-8''%E2%82%AC-invoice.pdf",
            "Content-Transfer-Encoding: base64",
            "",
            "JVBERi0x",
            "LjQ=",
            "--outer",
            "Content-Type: audio/ogg",
            "Content-Disposition: attachment",
            "Content-Transfer-Encoding: base64",
            "",
            "T2dnUw==",
            "--outer--",
        ]);
        let mail = parse(&raw);
        assert_eq!(mail.body, "Café & “tea”\n\n- one\n- two\nafter");
        assert_eq!(
            mail.attachments,
            ["€-invoice.pdf, application/pdf, 8 bytes"]
        );
        assert_eq!(
            mail.audio,
            [(
                "audio/ogg".to_string(),
                "voice.ogg".to_string(),
                b"OggS".to_vec()
            )]
        );
        assert_eq!(
            mail.said(),
            "Subject: Invoice\n\nCafé & “tea”\n\n- one\n- two\nafter\n(attached, not read: €-invoice.pdf, application/pdf, 8 bytes)"
        );
    }

    #[test]
    fn decoding_takes_mail_as_it_is_written() {
        assert_eq!(
            decode_words("=?ISO-8859-1?Q?Andr=E9?= Pirard"),
            "André Pirard"
        );
        assert_eq!(decode_words("=?utf-8?B?w6k=?=  =?utf-8?B?w6k=?= x"), "éé x");
        assert_eq!(decode_words("a =?bogus b"), "a =?bogus b");
        assert_eq!(decode_charset(&[0x80, 0x93, 0xE9], "windows-1252"), "€“é");
        assert_eq!(decode_charset(&[0xA4], "iso-8859-15"), "€");
        assert_eq!(base64_lenient(b"aGVs\r\nbG8=\r\n"), b"hello");
        // Blocks padded one after another, and a broken tail.
        assert_eq!(base64_lenient(b"aGk=aGk="), b"hihi");
        assert_eq!(base64_lenient(b"aGVsbG8"), b"hello");
        assert_eq!(quoted_printable(b"a=3Db=\r\nc =\nd"), b"a=bc d");
        assert_eq!(
            addresses("Group: \"Doe, J\" <J@X.org>, k@y.org (Kay);, <bad>"),
            ["j@x.org", "k@y.org"]
        );
        assert_eq!(unflow("one \r\ntwo\r\n", false), "one two\n\n");
        assert_eq!(message_ids("<a@b> junk <c@d>"), ["a@b", "c@d"]);
    }

    #[test]
    fn quoted_messages_and_signatures_are_cut() {
        assert_eq!(own_words("Yes.\n\n-- \nAlice\n"), "Yes.");
        assert_eq!(
            own_words("Fine.\n\n-----Original Message-----\nFrom: x\n\nold"),
            "Fine."
        );
        assert_eq!(
            own_words("Fine.\n________________________________\nFrom: Lead\nSent: …"),
            "Fine."
        );
        assert_eq!(
            own_words("Le lun. 26 oct. 2026, Lead a écrit :\n> old\n"),
            ""
        );
        // An attribution without a quote after it is what was said.
        assert_eq!(
            own_words("On Monday I wrote:\nthe plan"),
            "On Monday I wrote:\nthe plan"
        );
        assert_eq!(own_words("> what?\nThis.\n> more"), "This.");
    }

    #[test]
    fn machines_are_not_answered() {
        let read = |extra: &str| {
            parse(format!("From: a@example.org\r\n{extra}\r\n\r\nhi").as_bytes()).automatic
        };
        assert!(read("Auto-Submitted: auto-replied").is_some());
        assert!(read("Auto-Submitted: no").is_none());
        assert!(read("Precedence: bulk").is_some());
        assert!(read("List-Id: <team.example.org>").is_some());
        assert!(read("Return-Path: <>").is_some());
        assert!(
            read("Content-Type: multipart/report; report-type=delivery-status; boundary=x")
                .is_some()
        );
        assert!(parse(b"From: MAILER-DAEMON@example.org\r\n\r\nx")
            .automatic
            .is_some());
        assert!(read("X-Mailer: something").is_none());
    }

    #[test]
    fn only_the_providers_results_say_who_wrote() {
        let from = "alice@example.org";
        let gmail = GMAIL_RESULTS.to_string();
        assert_eq!(
            authenticated(std::slice::from_ref(&gmail), from, None),
            Ok(())
        );
        assert_eq!(
            authenticated(std::slice::from_ref(&gmail), from, Some("mx.google.com")),
            Ok(())
        );
        assert_eq!(
            authenticated(std::slice::from_ref(&gmail), from, Some("google.com")),
            Ok(())
        );
        // Another server's results are not the provider's.
        let forged = "evil.example; dmarc=pass header.from=example.org".to_string();
        assert!(authenticated(std::slice::from_ref(&forged), from, Some("mx.google.com")).is_err());
        // The topmost is the provider's: a failing one is not saved by a
        // passing one the sender wrote below it.
        let failing = "mx.google.com; dkim=none; spf=softfail smtp.mailfrom=example.org; dmarc=fail (p=NONE) header.from=example.org".to_string();
        let error = authenticated(&[failing, forged.clone()], from, None).unwrap_err();
        assert!(error.contains("DMARC fail"), "{error}");
        // DKIM for the domain the From is under, or SPF for it, is enough.
        let dkim = "mx.example.net; dkim=pass header.d=example.org".to_string();
        assert_eq!(
            authenticated(std::slice::from_ref(&dkim), "a@mail.example.org", None),
            Ok(())
        );
        assert!(authenticated(&[dkim], "a@notexample.org", None).is_err());
        let spf = "mx.example.net; spf=pass smtp.mailfrom=bounce@example.org".to_string();
        assert_eq!(authenticated(&[spf], from, None), Ok(()));
        // DKIM passing for another domain is not the From's.
        let other = "mx.example.net; dkim=pass header.d=elsewhere.example; dmarc=none".to_string();
        let error = authenticated(&[other], from, None).unwrap_err();
        assert!(
            error.contains("not shown to be from example.org"),
            "{error}"
        );
        // Microsoft's results name no server.
        let microsoft = "spf=pass (sender IP is 192.0.2.1) smtp.mailfrom=example.org; dkim=pass (signature was verified) header.d=example.org;dmarc=pass action=none header.from=example.org;compauth=pass reason=100".to_string();
        assert_eq!(authenticated(&[microsoft], from, None), Ok(()));
        assert!(authenticated(&[], from, None)
            .unwrap_err()
            .contains("EMAIL_TRUST_FROM"));
    }

    #[test]
    fn replies_are_written_to_be_read_and_threaded() {
        let thread = Thread {
            subject: "Café budget".into(),
            message_id: Some("m4@example.org".into()),
            references: vec!["m1@example.org".into(), "m4@example.org".into()],
        };
        let text = "100 USD left.\n.hidden line\nCafé ".to_string() + &"x".repeat(80);
        let raw = compose(
            "lead@example.net",
            Some("Lead, the agent"),
            "alice@example.org",
            Some(&thread),
            &text,
            None,
            1_793_000_000,
            "abc",
        );
        let written = String::from_utf8(raw.clone()).unwrap();
        assert!(
            written.starts_with(
                "From: \"Lead, the agent\" <lead@example.net>\r\nTo: <alice@example.org>\r\n"
            ),
            "{written}"
        );
        assert!(written.contains("\r\nSubject: =?utf-8?B?"), "{written}");
        assert!(
            written.contains("\r\nDate: Mon, 26 Oct 2026 07:33:20 +0000\r\n"),
            "{written}"
        );
        assert!(written.contains("\r\nMessage-ID: <abc@example.net>\r\n"));
        assert!(written.contains("\r\nIn-Reply-To: <m4@example.org>\r\n"));
        assert!(written.contains("\r\nReferences: <m1@example.org>\r\n <m4@example.org>\r\n"));
        assert!(written.contains("\r\nAuto-Submitted: auto-replied\r\n"));
        assert!(written.lines().all(|l| l.len() <= 78), "{written}");
        // Read back as a message, it says what was written.
        let read = parse(&raw);
        assert_eq!(read.subject, "Re: Café budget");
        assert_eq!(read.to, ["alice@example.org"]);
        assert_eq!(read.body, text.trim_end());
        assert!(read.automatic.is_some());
        // As DATA sends it, the dotted line is given another dot.
        let data = dot_stuffed(&raw);
        assert!(String::from_utf8_lossy(&data).contains("\r\n..hidden line\r\n"));
        assert!(data.ends_with(b"\r\n.\r\n"));
        assert_eq!(dot_stuffed(b".\nx\r"), b"..\r\nx\r\n.\r\n");
        // With audio, it is attached.
        let audio = Audio {
            media_type: "audio/ogg".into(),
            name: "reply.ogg".into(),
            bytes: b"OggS-spoken".to_vec(),
        };
        let raw = compose(
            "lead@example.net",
            None,
            "alice@example.org",
            None,
            "Said aloud.",
            Some(&audio),
            0,
            "v1",
        );
        let read = parse(&raw);
        assert_eq!(read.body, "Said aloud.");
        assert_eq!(read.subject, "Re: your message");
        assert_eq!(
            read.audio,
            [(
                "audio/ogg".into(),
                "reply.ogg".into(),
                b"OggS-spoken".to_vec()
            )]
        );
        assert_eq!(date(0), "Thu, 1 Jan 1970 00:00:00 +0000");
    }

    #[test]
    fn imap_answers_are_read_with_their_literals() {
        assert_eq!(literal_size("* 1 FETCH (UID 4 BODY[] {12}"), Some((12, 24)));
        assert_eq!(literal_size("* 1 FETCH (BODY[] {12+}"), Some((12, 18)));
        assert_eq!(literal_size("* OK {not}"), None);
        let said = Said {
            text: "* 3 FETCH (FLAGS (\\Seen \\Answered) UID 42 RFC822.SIZE 512 BODY[] {#0})".into(),
            literals: vec![b"Subject: x\r\n\r\nhi".to_vec()],
        };
        assert_eq!(
            fetched(&said),
            Some(Fetched {
                uid: Some(42),
                size: Some(512),
                body: Some(b"Subject: x\r\n\r\nhi".to_vec())
            })
        );
        assert_eq!(
            fetched(&Said {
                text: "* 7 EXISTS".into(),
                ..Default::default()
            }),
            None
        );
        assert_eq!(
            code_number("* OK [UIDVALIDITY 3857529045] UIDs valid", "UIDVALIDITY"),
            Some(3_857_529_045)
        );
        assert_eq!(
            bracketed_capabilities("* OK [CAPABILITY IMAP4rev1 STARTTLS auth=plain] hi"),
            Some(
                ["IMAP4REV1", "STARTTLS", "AUTH=PLAIN"]
                    .map(String::from)
                    .into_iter()
                    .collect()
            )
        );
        assert_eq!(quoted("a\"b\\c").ok().as_deref(), Some("\"a\\\"b\\\\c\""));
        assert!(quoted("é").is_err());
        assert_eq!(
            mechanisms(&[
                "smtp.example.org".into(),
                "AUTH PLAIN LOGIN".into(),
                "AUTH=XOAUTH2".into()
            ]),
            ["LOGIN", "PLAIN", "XOAUTH2"]
                .map(String::from)
                .into_iter()
                .collect()
        );
        assert_eq!(STANDARD.decode(plain("u", "p")).unwrap(), b"\0u\0p");
        assert_eq!(
            STANDARD.decode(xoauth2("u@x.org", "t0k")).unwrap(),
            b"user=u@x.org\x01auth=Bearer t0k\x01\x01"
        );
    }

    #[test]
    fn nothing_is_sent_in_the_clear_but_to_the_machine_itself() {
        for host in ["127.0.0.1", "::1", "localhost", "imap.localhost"] {
            assert!(is_loopback(host), "{host}");
        }
        for host in [
            "imap.example.org",
            "10.0.0.1",
            "192.168.1.2",
            "localhost.example.org",
        ] {
            assert!(!is_loopback(host), "{host}");
        }
        assert_eq!(url_host("imap://[::1]:143").as_deref(), Some("::1"));
        assert_eq!(
            url_host("smtps://Smtp.Example.org:465/").as_deref(),
            Some("smtp.example.org")
        );
        assert!(valid_address("a.b+c@example.org"));
        for bad in [
            "a@b",
            "<a@example.org>",
            "a b@example.org",
            "a@example.org\r\nRCPT",
            "@example.org",
        ] {
            assert!(!valid_address(bad), "{bad}");
        }
    }
}
