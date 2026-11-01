//! The bridge between a messenger and a profile ([`run`]): what it takes
//! from the messenger's chats, and how it answers them.
//!
//! It receives on a thread of its own, and answers each chat on another,
//! [`Options::parallel`] chats at once and one turn at a time in each. What
//! a chat says while the agent works on it is told to the agent as it
//! works, queued, collected, or made to interrupt it ([`Busy`]); `/stop`
//! stops it at once. The messages of a burst are one turn
//! ([`Options::debounce_ms`]). Each message is marked taken, and then
//! answered or failed, where the messenger can ([`Messenger::acknowledge`]).
//!
//! The pictures a person sends are shown to the agent, and every file they
//! send is put in its workspace's inbox ([`crate::files::receive`]); the
//! files the agent shares go back with its reply, where the messenger takes
//! files ([`Messenger::send_file`]).
//!
//! A reply is sent again after a failure that may pass ([`ATTEMPTS`]), and
//! after [`TRIP`] failures in a row nothing is sent to the messenger for a
//! while ([`Breaker`]). A reply not sent is kept in an outbox and sent once
//! the messenger takes messages again, after a restart too. What the bridge
//! took and had not answered when it stopped is answered when it starts
//! again, and a chat whose answer it cut off is told so.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::channels::{is_decision, is_privileged, set_state, state, MAX_MESSAGE};
use crate::error::CliError;
use crate::messengers::{Ack, Busy, Incoming, Messenger, Options, Streaming, POLL_PAUSE};
use crate::profiles::Profile;
use crate::runtime::{Hooks, SharedLedger};

/// Tries of a reply before it waits in the outbox.
pub const ATTEMPTS: u32 = 3;

/// The pause after a reply's first failed try, doubled after each.
const RETRY_PAUSE: Duration = Duration::from_millis(500);

/// Failed sends in a row after which a messenger's breaker opens.
pub const TRIP: u32 = 5;

/// How long a breaker stays open the first time; twice as long each time
/// it opens again before a send goes through.
pub const OPEN_FOR: Duration = Duration::from_secs(60);

/// The longest a breaker stays open.
pub const OPEN_AT_MOST: Duration = Duration::from_secs(600);

/// The most replies an outbox keeps: past that, the oldest is dropped.
pub const OUTBOX_MOST: usize = 200;

/// How often the outbox is tried while it holds replies.
const OUTBOX_EVERY: Duration = Duration::from_secs(5);

/// How long a burst is gathered at most, in pauses of
/// [`Options::debounce_ms`].
const GATHER_MOST: u32 = 8;

/// The most messages one turn takes together.
const TURN_MOST: usize = 20;

/// The longest pause between two tries of a messenger that fails to
/// receive, in seconds.
const RECEIVE_PAUSE_MOST: u64 = 60;

/// What a chat is told whose answer the bridge cut off when it stopped.
pub const RESTARTED: &str =
    "I was restarted while I worked on what you sent, so I am taking it up again.";

/// What an answer becomes that a message interrupted before it said
/// anything ([`Busy::Interrupt`]).
const INTERRUPTED: &str = "(Stopped, to take up what you said next.)";

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// A messenger's circuit breaker: after [`TRIP`] failed sends in a row it
/// opens, and no send is tried until it closes again. The first send after
/// that decides: if it goes through, the breaker closes; if it fails, the
/// breaker opens again, for twice as long ([`OPEN_FOR`], at most
/// [`OPEN_AT_MOST`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Breaker {
    failures: u32,
    open_until: Option<Instant>,
    open_for: Duration,
}

impl Default for Breaker {
    fn default() -> Breaker {
        Breaker {
            failures: 0,
            open_until: None,
            open_for: OPEN_FOR,
        }
    }
}

impl Breaker {
    /// Whether a send may be tried at `now`.
    pub fn allows(&self, now: Instant) -> bool {
        self.open_until.map_or(true, |until| now >= until)
    }

    /// A send went through: the breaker closes.
    pub fn sent(&mut self) {
        *self = Breaker::default();
    }

    /// A send failed at `now` with an error that may pass: whether the
    /// breaker opened.
    pub fn failed(&mut self, now: Instant) -> bool {
        self.failures += 1;
        // The first send after it was open failed too: open again.
        let tried_again = self.open_until.is_some();
        if tried_again || self.failures >= TRIP {
            self.open_until = Some(now + self.open_for);
            self.open_for = (self.open_for * 2).min(OPEN_AT_MOST);
            true
        } else {
            false
        }
    }

    /// Until when it is open, if it is at `now`.
    pub fn open_until(&self, now: Instant) -> Option<Instant> {
        self.open_until.filter(|until| *until > now)
    }
}

/// What a failure's text says may pass: the messenger was busy or briefly
/// down, rather than refusing the message.
const PASSING: [&str; 10] = [
    "ratelimited",
    "rate limit",
    "too many",
    "timeout",
    "timed out",
    "temporar",
    "unavailable",
    "internal_error",
    "fatal_error",
    "try again",
];

/// Whether a failed send failed for good: the messenger answered and
/// refused the message itself (a client error, other than one for too many
/// requests or for a request that took too long), so sending it again would
/// not help.
pub fn lasting(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    if PASSING.iter().any(|passing| lower.contains(passing)) {
        return false;
    }
    if let Some(status) = http_status(error) {
        return (400..500).contains(&status) && !matches!(status, 408 | 425 | 429);
    }
    !lower.contains("connection refused")
        && (lower.starts_with("refused: ") || lower.contains(" refused "))
}

/// The HTTP status an error names (`HTTP 403: …`), if it names one.
fn http_status(error: &str) -> Option<u16> {
    let at = error.find("HTTP ")?;
    error.get(at + 5..at + 8)?.parse().ok()
}

/// A reply not sent yet, and the message it answers (without its text):
/// where it goes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Held {
    /// The message it answers.
    pub to: Incoming,
    /// The reply.
    pub text: String,
}

/// What became of a reply ([`Delivery::reply`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sent {
    /// The messenger took it.
    Delivered,
    /// It waits in the outbox: the messenger failed, or its breaker is open.
    Held(String),
    /// The messenger refused it, and would again: it is dropped.
    Refused(String),
}

/// Why a send did not go through.
enum Failure {
    /// The messenger refused it ([`lasting`]).
    Lasting(String),
    /// The messenger failed, or its breaker is open.
    Passing(String),
}

/// How a bridge's replies reach its messenger: sent again after failures
/// that may pass, held back while the messenger's [`Breaker`] is open, and
/// kept in an outbox (in the channel state) while they cannot be sent, each
/// chat's in the order they were written.
#[derive(Debug)]
pub struct Delivery {
    key: String,
    breaker: Mutex<Breaker>,
    outbox: Mutex<VecDeque<Held>>,
    /// Held while the outbox is sent: by one thread at a time.
    flushing: Mutex<()>,
}

impl Delivery {
    /// The delivery whose outbox is kept in the channel state under `key`,
    /// holding what it held when it was last used.
    pub fn load(key: &str) -> Result<Delivery, CliError> {
        let outbox = state(key)?
            .and_then(|kept| serde_json::from_str(&kept).ok())
            .unwrap_or_default();
        Ok(Delivery {
            key: key.to_string(),
            breaker: Mutex::default(),
            outbox: Mutex::new(outbox),
            flushing: Mutex::new(()),
        })
    }

    /// Sends `text` through `messenger` in answer to `message`: tried
    /// [`ATTEMPTS`] times, and kept in the outbox if it does not go
    /// through, or if earlier replies to its chat wait there.
    pub fn reply(&self, messenger: &dyn Messenger, message: &Incoming, text: &str) -> Sent {
        if self.holds(&message.chat) {
            self.flush(messenger);
            if self.holds(&message.chat) {
                self.hold(message, text);
                return Sent::Held("earlier replies to this chat wait to be sent".into());
            }
        }
        match self.send(|| messenger.reply(message, text)) {
            Ok(()) => Sent::Delivered,
            Err(Failure::Lasting(why)) => Sent::Refused(why),
            Err(Failure::Passing(why)) => {
                self.hold(message, text);
                Sent::Held(why)
            }
        }
    }

    /// Sends `file` through `messenger` in answer to `message`, tried as a
    /// reply is but not kept if it does not go through (a file is too big
    /// for the outbox): whether the messenger sends files at all.
    pub fn file(
        &self,
        messenger: &dyn Messenger,
        message: &Incoming,
        file: &crate::files::File,
    ) -> Result<bool, String> {
        let taken = std::cell::Cell::new(true);
        match self.send(|| {
            messenger
                .send_file(message, file, "")
                .map(|sent| taken.set(sent))
        }) {
            Ok(()) => Ok(taken.get()),
            Err(Failure::Lasting(why) | Failure::Passing(why)) => Err(why),
        }
    }

    /// Tries `send` until it goes through, fails for good, has failed
    /// [`ATTEMPTS`] times, or opens the breaker.
    fn send(&self, send: impl Fn() -> Result<(), String>) -> Result<(), Failure> {
        let mut pause = RETRY_PAUSE;
        let mut tries = 0;
        loop {
            if !lock(&self.breaker).allows(Instant::now()) {
                return Err(Failure::Passing(
                    "the messenger failed too often: nothing is sent to it for a while".into(),
                ));
            }
            tries += 1;
            match send() {
                Ok(()) => {
                    lock(&self.breaker).sent();
                    return Ok(());
                }
                // Refused, the messenger did not fail: the breaker does not
                // count it.
                Err(why) if lasting(&why) => return Err(Failure::Lasting(why)),
                Err(why) => {
                    let opened = lock(&self.breaker).failed(Instant::now());
                    if opened {
                        println!(
                            "  (the messenger fails, so nothing is sent to it for a while: {why})"
                        );
                    }
                    if opened || tries >= ATTEMPTS {
                        return Err(Failure::Passing(why));
                    }
                    std::thread::sleep(pause);
                    pause *= 2;
                }
            }
        }
    }

    fn holds(&self, chat: &str) -> bool {
        lock(&self.outbox).iter().any(|held| held.to.chat == chat)
    }

    fn hold(&self, message: &Incoming, text: &str) {
        let mut outbox = lock(&self.outbox);
        outbox.push_back(Held {
            to: Incoming {
                text: String::new(),
                voice: None,
                mentions: Vec::new(),
                files: Vec::new(),
                ..message.clone()
            },
            text: text.to_string(),
        });
        while outbox.len() > OUTBOX_MOST {
            if let Some(dropped) = outbox.pop_front() {
                println!(
                    "  (the outbox is full: a reply to {} is dropped)",
                    dropped.to.chat
                );
            }
        }
        self.keep(&outbox);
    }

    fn keep(&self, outbox: &VecDeque<Held>) {
        let kept = (!outbox.is_empty()).then(|| serde_json::to_string(outbox).unwrap_or_default());
        if let Err(e) = set_state(&self.key, kept.as_deref()) {
            println!("  (the outbox could not be kept: {e})");
        }
    }

    /// Sends what the outbox holds, oldest first, until the messenger
    /// fails: how many replies went.
    pub fn flush(&self, messenger: &dyn Messenger) -> usize {
        // Another thread is at it.
        let Ok(_flushing) = self.flushing.try_lock() else {
            return 0;
        };
        let mut sent = 0;
        loop {
            let Some(next) = lock(&self.outbox).front().cloned() else {
                return sent;
            };
            match self.send(|| messenger.reply(&next.to, &next.text)) {
                Ok(()) => {
                    println!("to {} (kept until now): {}", next.to.chat, next.text);
                    sent += 1;
                }
                Err(Failure::Lasting(why)) => {
                    println!(
                        "  (a kept reply to {} is refused, and dropped: {why})",
                        next.to.chat
                    );
                }
                Err(Failure::Passing(_)) => return sent,
            }
            let mut outbox = lock(&self.outbox);
            if outbox.front() == Some(&next) {
                outbox.pop_front();
            }
            self.keep(&outbox);
        }
    }

    /// Whether the outbox holds replies.
    pub fn holding(&self) -> bool {
        !lock(&self.outbox).is_empty()
    }
}

/// The conversation `message` is part of, with `profile` on `messenger`:
/// one per chat, and one per thread where the messenger keeps threads apart
/// (a Slack thread, a Telegram topic, a Mattermost thread).
pub fn conversation(messenger: &str, profile: &str, message: &Incoming) -> String {
    if message.thread.is_empty() {
        format!("{messenger}:{profile}:{}", message.chat)
    } else {
        format!("{messenger}:{profile}:{}:{}", message.chat, message.thread)
    }
}

/// Whether `text` is a command, which a turn takes alone.
fn is_command(text: &str) -> bool {
    text.trim_start().starts_with('/')
}

/// A message waiting to be answered.
#[derive(Clone, Debug)]
struct Waiting {
    /// Its place in the order the bridge took messages.
    seq: u64,
    message: Incoming,
    /// Whether it came while the agent worked on its chat.
    queued: bool,
    /// The burst it came in: the first message of the burst's place
    /// ([`Options::debounce_ms`]).
    burst: u64,
    /// Its answer, if the bridge has it already (a refusal, say): no run.
    canned: Option<String>,
    /// Whether that answer is the sender's alone, not printed (a pairing
    /// code).
    private: bool,
}

/// Whether the bridge answers a message ([`Bridge::admits`]).
enum Admitted {
    /// It does.
    Yes,
    /// It does not.
    No,
    /// It gives its sender this, a pairing code, in its turn.
    Code(String),
}

/// The next turn of a chat, taken from what waits in it: a command, or a
/// message whose answer is known, alone; or the texts that follow one
/// another, together where they came in one burst, or came while the agent
/// worked and `busy` collects them (`collect`, and `interrupt`, whose
/// messages after the first come while the agent stops).
fn next_turn(waiting: &mut VecDeque<Waiting>, busy: Busy) -> Vec<Waiting> {
    let Some(first) = waiting.pop_front() else {
        return Vec::new();
    };
    let alone = |w: &Waiting| w.canned.is_some() || is_command(&w.message.text);
    if alone(&first) {
        return vec![first];
    }
    let collects = matches!(busy, Busy::Collect | Busy::Interrupt);
    let goes_with = |w: &Waiting| w.burst == first.burst || (collects && first.queued && w.queued);
    let mut size = first.message.text.len();
    let mut turn = vec![first.clone()];
    while let Some(next) = waiting.front() {
        let more = next.message.text.len() + 1;
        if alone(next)
            || !goes_with(next)
            || turn.len() >= TURN_MOST
            || size + more > MAX_MESSAGE / 2
        {
            break;
        }
        size += more;
        turn.extend(waiting.pop_front());
    }
    turn
}

/// What a turn's messages say to the agent, together: each on a line of
/// its own, marked with who wrote it where more than one person did.
fn together(heard: &[(&Incoming, String)]) -> String {
    let one_writer = heard
        .windows(2)
        .all(|pair| pair[0].0.from == pair[1].0.from);
    heard
        .iter()
        .map(|(message, text)| {
            if one_writer || message.from.is_empty() {
                text.clone()
            } else {
                format!("{}: {text}", message.from)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A chat, as the dispatcher sees it: what waits in it, and the turn being
/// answered.
#[derive(Default)]
struct Chat {
    waiting: VecDeque<Waiting>,
    /// When what waits is taken up, unless more comes first.
    ready_at: Option<Instant>,
    /// When the first of what waits came: a burst is gathered so long at
    /// most.
    first_at: Option<Instant>,
    /// The turn being answered, if one is.
    live: Option<Arc<Live>>,
}

impl Chat {
    fn idle(&self) -> bool {
        self.live.is_none() && self.waiting.is_empty()
    }

    fn ready(&self, now: Instant) -> bool {
        self.live.is_none()
            && !self.waiting.is_empty()
            && self.ready_at.map_or(true, |at| at <= now)
    }
}

/// What the dispatcher and a turn's thread share while the agent works.
#[derive(Default)]
struct Live {
    /// What the chat said meanwhile, for the agent to take in
    /// ([`Busy::Steer`]).
    said: Mutex<Vec<Waiting>>,
    /// What the agent took in of it: answered with the turn.
    told: Mutex<Vec<Waiting>>,
    /// `/stop` was said.
    stopped: AtomicBool,
    /// A message interrupted the run ([`Busy::Interrupt`]).
    interrupted: AtomicBool,
}

/// What reaches the dispatcher.
enum Event {
    /// Messages the bridge took, in order.
    Took(Vec<(u64, Incoming)>),
    /// Receiving ended: after its one batch (`once`), or failing.
    Received(Result<(), String>),
    /// A turn was answered.
    Answered(Finished),
}

/// What a turn's thread tells the dispatcher at its end.
struct Finished {
    key: String,
    /// The messages it answered.
    answered: Vec<u64>,
    /// What the chat said meanwhile that the agent did not take in: the
    /// next turn.
    unheard: Vec<Waiting>,
}

/// What the bridge does besides answering, in order, on a thread of its
/// own.
enum Aside {
    /// Marks a message.
    Mark(Incoming, Ack),
    /// Says something in answer to a message, at once.
    Say(Incoming, String),
    /// Sends what the outbox holds.
    Flush,
}

/// What a bridge took and has not answered yet, kept in the channel state
/// so that a restart answers it; and the conversations it is answering,
/// which are told after a restart that their answer was cut off.
#[derive(Default)]
struct Taken {
    pending: BTreeMap<u64, Incoming>,
    answering: BTreeSet<String>,
}

/// A bridge at work.
struct Bridge<'a> {
    ledger: SharedLedger,
    profile: &'a Profile,
    messenger: &'a dyn Messenger,
    options: &'a Options,
    word: &'static str,
    /// Where its pairings are kept: `messenger:profile`.
    channel: String,
    cursor_key: String,
    delivery: Delivery,
    taken: Mutex<Taken>,
    seq: AtomicU64,
    /// The names the bot goes by, once asked.
    me: OnceLock<Vec<String>>,
    /// The chats said to be ignored, each once.
    ignored: Mutex<BTreeSet<String>>,
    /// Whether marking a message failed already: it is said once.
    marking_failed: AtomicBool,
    /// Once set, no more messages are taken.
    stop: &'a AtomicBool,
}

/// Answers the chats `options` allow with `profile` through `messenger`,
/// until stopped; with `options.once`, the messages of one batch, then
/// returns.
pub fn run(
    ledger: SharedLedger,
    profile: &Profile,
    messenger: &dyn Messenger,
    options: &Options,
) -> Result<(), CliError> {
    run_until(ledger, profile, messenger, options, &AtomicBool::new(false))
}

/// [`run`], until `stop` is set: then the bridge takes no more messages,
/// answers what it took, and returns.
pub fn run_until(
    ledger: SharedLedger,
    profile: &Profile,
    messenger: &dyn Messenger,
    options: &Options,
    stop: &AtomicBool,
) -> Result<(), CliError> {
    if options.allow.is_empty() && options.routes.is_empty() && !options.pairing {
        return Err(CliError::Usage(
            "name the chats the profile answers (--allow <chat>,…), or let people pair (--pairing)"
                .into(),
        ));
    }
    if options.pairing && !messenger.sees_strangers() {
        return Err(CliError::Usage(if messenger.name() == "slack" {
            "slack is polled here channel by channel, so no message from a chat it does not answer reaches the bridge: give Slack an app-level token (SLACK_APP_TOKEN) for Socket Mode".to_string()
        } else {
            format!(
                "{} cannot pair: nothing it passes on is known to be a direct chat with someone it does not answer yet; name the chats (--allow)",
                messenger.name()
            )
        }));
    }
    let cursor_key = messenger.cursor_key(&profile.name);
    let bridge = Bridge {
        ledger,
        profile,
        messenger,
        options,
        word: messenger.chat_word(),
        channel: format!("{}:{}", messenger.name(), profile.name),
        delivery: Delivery::load(&format!("{cursor_key}:outbox"))?,
        cursor_key,
        taken: Mutex::default(),
        seq: AtomicU64::new(0),
        me: OnceLock::new(),
        ignored: Mutex::default(),
        marking_failed: AtomicBool::new(false),
        stop,
    };
    // What was taken and not answered when the bridge last stopped, which
    // is answered first; and the chats whose answer it cut off.
    let carried: Vec<Incoming> = state(&bridge.pending_key())?
        .and_then(|kept| serde_json::from_str(&kept).ok())
        .unwrap_or_default();
    let cut_off: BTreeSet<String> = state(&bridge.answering_key())?
        .and_then(|kept| serde_json::from_str(&kept).ok())
        .unwrap_or_default();
    let carried: Vec<(u64, Incoming)> = carried
        .into_iter()
        .map(|message| (bridge.seq.fetch_add(1, Ordering::SeqCst), message))
        .collect();
    let restarted: BTreeSet<String> = carried
        .iter()
        .map(|(_, message)| bridge.key(message))
        .filter(|key| cut_off.contains(key))
        .collect();
    bridge.keep(&carried)?;
    set_state(&bridge.answering_key(), None)?;
    if !carried.is_empty() {
        println!(
            "{}: {} message(s) taken before the bridge stopped, answered now",
            messenger.name(),
            carried.len()
        );
    }
    bridge.dispatch(carried, restarted)
}

impl Bridge<'_> {
    fn pending_key(&self) -> String {
        format!("{}:pending", self.cursor_key)
    }

    fn answering_key(&self) -> String {
        format!("{}:answering", self.cursor_key)
    }

    fn key(&self, message: &Incoming) -> String {
        conversation(
            self.messenger.name(),
            &self.profile_for(message).name,
            message,
        )
    }

    /// The profile that answers `message`'s chat: its route's, or the
    /// bridge's.
    fn profile_for(&self, message: &Incoming) -> &Profile {
        self.options
            .routes
            .get(&message.chat)
            .unwrap_or(self.profile)
    }

    /// Keeps `messages` among what was taken and not answered.
    fn keep(&self, messages: &[(u64, Incoming)]) -> Result<(), CliError> {
        let mut taken = lock(&self.taken);
        for (seq, message) in messages {
            taken.pending.insert(*seq, message.clone());
        }
        self.save_pending(&taken)
    }

    /// Lets go of what was answered.
    fn forget(&self, answered: &[u64]) {
        let mut taken = lock(&self.taken);
        for seq in answered {
            taken.pending.remove(seq);
        }
        if let Err(e) = self.save_pending(&taken) {
            println!("  (what the bridge took could not be kept: {e})");
        }
    }

    fn save_pending(&self, taken: &Taken) -> Result<(), CliError> {
        let pending: Vec<&Incoming> = taken.pending.values().collect();
        let kept =
            (!pending.is_empty()).then(|| serde_json::to_string(&pending).unwrap_or_default());
        set_state(&self.pending_key(), kept.as_deref())
    }

    /// Notes that the agent works, or no longer works, on conversation
    /// `key`.
    fn answering(&self, key: &str, working: bool) {
        let mut taken = lock(&self.taken);
        if working {
            taken.answering.insert(key.to_string());
        } else {
            taken.answering.remove(key);
        }
        let kept = (!taken.answering.is_empty())
            .then(|| serde_json::to_string(&taken.answering).unwrap_or_default());
        if let Err(e) = set_state(&self.answering_key(), kept.as_deref()) {
            println!("  (what the bridge works on could not be kept: {e})");
        }
    }

    /// Receives, and passes on what it took, until it fails (the first
    /// time it tries, or with `once`) or, with `once`, after one batch.
    /// Each message is kept before the cursor moves past it, so that it is
    /// answered after a restart if the bridge stops first.
    fn receive(&self, events: &Sender<Event>) {
        let mut failures: u32 = 0;
        let mut first = true;
        loop {
            if self.stop.load(Ordering::SeqCst) {
                let _ = events.send(Event::Received(Ok(())));
                return;
            }
            let cursor = match state(&self.cursor_key) {
                Ok(cursor) => cursor.unwrap_or_default(),
                Err(e) => {
                    let _ = events.send(Event::Received(Err(e.to_string())));
                    return;
                }
            };
            let wait = if self.options.once {
                self.messenger.once_wait()
            } else {
                50
            };
            match self.messenger.receive(&cursor, wait) {
                Ok((messages, next)) => {
                    failures = 0;
                    first = false;
                    let numbered: Vec<(u64, Incoming)> = messages
                        .into_iter()
                        .map(|message| (self.seq.fetch_add(1, Ordering::SeqCst), message))
                        .collect();
                    // Written only when it changes: a quiet poll writes nothing.
                    let kept = if numbered.is_empty() {
                        Ok(())
                    } else {
                        self.keep(&numbered)
                    };
                    if let Err(e) = kept.and_then(|()| {
                        if next == cursor {
                            Ok(())
                        } else {
                            set_state(&self.cursor_key, Some(&next))
                        }
                    }) {
                        let _ = events.send(Event::Received(Err(e.to_string())));
                        return;
                    }
                    let quiet = numbered.is_empty();
                    if !quiet {
                        let _ = events.send(Event::Took(numbered));
                    }
                    if self.options.once {
                        let _ = events.send(Event::Received(Ok(())));
                        return;
                    }
                    if quiet && !self.messenger.long_polls() {
                        self.pause(Duration::from_secs(POLL_PAUSE));
                    }
                }
                Err(why) if first || self.options.once => {
                    let _ = events.send(Event::Received(Err(why)));
                    return;
                }
                Err(why) => {
                    failures += 1;
                    let pause = (1u64 << failures.min(6)).min(RECEIVE_PAUSE_MOST);
                    println!(
                        "{}: could not receive ({why}); trying again in {pause} s",
                        self.messenger.name()
                    );
                    self.pause(Duration::from_secs(pause));
                }
            }
        }
    }

    /// Waits `long`, or until the bridge is stopped.
    fn pause(&self, long: Duration) {
        let until = Instant::now() + long;
        while !self.stop.load(Ordering::SeqCst) {
            let now = Instant::now();
            if now >= until {
                return;
            }
            std::thread::sleep((until - now).min(Duration::from_millis(100)));
        }
    }

    /// Answers what was carried over and what is received: each chat's
    /// turns in order, chats side by side.
    fn dispatch(
        &self,
        carried: Vec<(u64, Incoming)>,
        mut restarted: BTreeSet<String>,
    ) -> Result<(), CliError> {
        let options = self.options;
        let parallel = options.parallel.max(1);
        std::thread::scope(|scope| {
            let (events, inbox) = mpsc::channel::<Event>();
            let (asides, aside_inbox) = mpsc::channel::<Aside>();
            scope.spawn(move || self.do_asides(aside_inbox));
            // With `once`, what was carried over is the batch.
            let mut receiving = !(options.once && !carried.is_empty());
            if receiving {
                let events = events.clone();
                scope.spawn(move || self.receive(&events));
            }
            let mut chats: BTreeMap<String, Chat> = BTreeMap::new();
            let mut failed: Option<String> = None;
            let mut flush_at = Instant::now() + OUTBOX_EVERY;
            if self.delivery.holding() {
                let _ = asides.send(Aside::Flush);
            }
            let now = Instant::now();
            for (seq, message) in carried {
                self.took(seq, message, &mut chats, &asides, now);
            }
            loop {
                let now = Instant::now();
                // The turns that are ready start, the chat that waited
                // longest first.
                while chats.values().filter(|chat| chat.live.is_some()).count() < parallel {
                    let Some(key) = chats
                        .iter()
                        .filter(|(_, chat)| chat.ready(now))
                        .min_by_key(|(_, chat)| chat.waiting.front().map_or(u64::MAX, |w| w.seq))
                        .map(|(key, _)| key.clone())
                    else {
                        break;
                    };
                    let Some(chat) = chats.get_mut(&key) else {
                        break;
                    };
                    let turn = next_turn(&mut chat.waiting, options.busy);
                    chat.first_at = None;
                    chat.ready_at = (!chat.waiting.is_empty()).then_some(now);
                    let live = Arc::new(Live::default());
                    chat.live = Some(live.clone());
                    self.answering(&key, true);
                    let notice = restarted.remove(&key);
                    let (events, asides) = (events.clone(), asides.clone());
                    scope.spawn(move || {
                        let finished = self.answer(key, turn, &live, notice, &asides);
                        let _ = events.send(Event::Answered(finished));
                    });
                }
                if !receiving && chats.values().all(Chat::idle) {
                    break;
                }
                if now >= flush_at {
                    if self.delivery.holding() {
                        let _ = asides.send(Aside::Flush);
                    }
                    flush_at = now + OUTBOX_EVERY;
                }
                let wake = chats
                    .values()
                    .filter(|chat| chat.live.is_none() && !chat.waiting.is_empty())
                    .filter_map(|chat| chat.ready_at)
                    .chain([flush_at])
                    .min()
                    .unwrap_or(flush_at);
                match inbox.recv_timeout(wake.saturating_duration_since(now)) {
                    Ok(Event::Took(messages)) => {
                        let now = Instant::now();
                        for (seq, message) in messages {
                            self.took(seq, message, &mut chats, &asides, now);
                        }
                    }
                    Ok(Event::Received(result)) => {
                        receiving = false;
                        failed = result.err();
                    }
                    Ok(Event::Answered(finished)) => {
                        self.finished(finished, &mut chats, Instant::now());
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    // Not while `events` is held here.
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
            drop(asides);
            match failed {
                Some(why) => Err(CliError::Connection(why)),
                None => Ok(()),
            }
        })
    }

    /// Takes up a message the bridge took: lets it in or not, stops what
    /// it says to stop, and otherwise has it answered as the chat's
    /// business allows.
    fn took(
        &self,
        seq: u64,
        message: Incoming,
        chats: &mut BTreeMap<String, Chat>,
        asides: &Sender<Aside>,
        now: Instant,
    ) {
        let code = match self.admits(&message) {
            Ok(Admitted::Yes) => None,
            Ok(Admitted::Code(code)) => Some(code),
            Ok(Admitted::No) => return self.forget(&[seq]),
            Err(e) => {
                println!("{} {}: {e}", self.word, message.chat);
                return self.forget(&[seq]);
            }
        };
        let key = self.key(&message);
        // A stranger's pairing code goes in its turn, as any answer.
        if let Some(code) = code {
            let chat = chats.entry(key).or_default();
            chat.waiting.push_back(Waiting {
                seq,
                message,
                queued: chat.live.is_some(),
                burst: seq,
                canned: Some(code),
                private: true,
            });
            chat.ready_at.get_or_insert(now);
            return;
        }
        self.print(&message);
        let text = message.text.trim();
        let command = text
            .split_whitespace()
            .next()
            .filter(|word| word.starts_with('/'))
            .unwrap_or_default()
            .to_string();
        let chat = chats.entry(key.clone()).or_default();
        if command == "/stop" {
            // What waits is dropped, and the agent stops.
            let mut dropped: Vec<Waiting> = chat.waiting.drain(..).collect();
            chat.ready_at = None;
            chat.first_at = None;
            let said = match &chat.live {
                Some(live) => {
                    live.stopped.store(true, Ordering::SeqCst);
                    crate::channels::stop(&key);
                    dropped.extend(lock(&live.said).drain(..));
                    // Its answer says that it stopped.
                    None
                }
                None if !dropped.is_empty() => {
                    Some("Stopped: what you sent before is dropped.".to_string())
                }
                None => Some("Nothing is running.".to_string()),
            };
            for waiting in &dropped {
                self.mark(asides, &waiting.message, Ack::Done);
            }
            let mut answered: Vec<u64> = dropped.iter().map(|w| w.seq).collect();
            answered.push(seq);
            self.forget(&answered);
            if let Some(said) = said {
                let _ = asides.send(Aside::Say(message, said));
            }
            return;
        }
        // Answered by the bridge, in its turn.
        let canned = if command == "/whoami" {
            Some(self.whoami(&message))
        } else if is_privileged(text) && !self.may_decide(&message) {
            let what = if is_decision(text) {
                "approve or decline"
            } else {
                "choose the model"
            };
            Some(format!(
                "Only {} may {what} here.",
                self.options.approvers.join(", ")
            ))
        } else {
            None
        };
        if canned.is_none() {
            self.mark(asides, &message, Ack::Seen);
        }
        // Part of the burst being gathered, or the first of a new one.
        let gathering = chat.live.is_none() && chat.ready_at.is_some_and(|at| at > now);
        let burst = match chat.waiting.back() {
            Some(last)
                if gathering
                    && canned.is_none()
                    && command.is_empty()
                    && last.canned.is_none()
                    && !is_command(&last.message.text) =>
            {
                last.burst
            }
            _ => seq,
        };
        let waiting = Waiting {
            seq,
            message,
            queued: chat.live.is_some(),
            burst,
            canned,
            private: false,
        };
        let busy = self.options.busy;
        match chat.live.clone() {
            Some(live)
                if busy == Busy::Steer
                    && command.is_empty()
                    && waiting.canned.is_none()
                    && waiting.message.voice.is_none() =>
            {
                lock(&live.said).push(waiting);
            }
            Some(live) => {
                if busy == Busy::Interrupt && command.is_empty() && waiting.canned.is_none() {
                    live.interrupted.store(true, Ordering::SeqCst);
                    crate::channels::stop(&key);
                }
                chat.waiting.push_back(waiting);
            }
            None => {
                chat.waiting.push_back(waiting);
                // A burst is gathered: taken up once nothing more comes for
                // a while.
                let debounce = Duration::from_millis(self.options.debounce_ms);
                let first = *chat.first_at.get_or_insert(now);
                chat.ready_at = Some((now + debounce).min(first + debounce * GATHER_MOST));
            }
        }
    }

    /// A turn's end: what it answered is let go, and what the agent did
    /// not take in is the chat's next turn.
    fn finished(&self, finished: Finished, chats: &mut BTreeMap<String, Chat>, now: Instant) {
        self.forget(&finished.answered);
        self.answering(&finished.key, false);
        let Some(chat) = chats.get_mut(&finished.key) else {
            return;
        };
        chat.live = None;
        // What the agent did not take in is one turn: said while it worked.
        let burst = finished.unheard.first().map(|w| w.seq);
        for unheard in finished.unheard {
            let unheard = Waiting {
                queued: false,
                burst: burst.unwrap_or(unheard.seq),
                ..unheard
            };
            let at = chat
                .waiting
                .iter()
                .position(|w| w.seq > unheard.seq)
                .unwrap_or(chat.waiting.len());
            chat.waiting.insert(at, unheard);
        }
        if chat.waiting.is_empty() {
            chats.remove(&finished.key);
        } else {
            chat.ready_at = Some(now);
            chat.first_at = None;
        }
    }

    /// Whether the bridge answers `message`: it comes from a chat the
    /// bridge answers, or from a person who paired with it, and, in a group
    /// where it must, it is said to the bot. A stranger is given a pairing
    /// code.
    fn admits(&self, message: &Incoming) -> Result<Admitted, CliError> {
        let options = self.options;
        if !options.allow.contains(&message.chat) && !options.routes.contains_key(&message.chat) {
            // Pairing lets in a person, in a chat of their own with the
            // bot: a group the bridge does not answer stays ignored.
            let pairs = options.pairing && message.direct;
            if !pairs {
                if lock(&self.ignored).insert(message.chat.clone()) {
                    println!("{} {}: not allowed, ignored", self.word, message.chat);
                }
                return Ok(Admitted::No);
            }
            if crate::pairing::load()?.allows(&self.channel, &message.chat) {
                return Ok(Admitted::Yes);
            }
            return match crate::pairing::stranger(
                &self.channel,
                &message.chat,
                &message.from,
                &message.text,
            )? {
                crate::pairing::Stranger::Allowed => Ok(Admitted::Yes),
                crate::pairing::Stranger::Code(code) => {
                    // The code goes to the sender only: the bridge does not
                    // print it.
                    println!(
                        "{} {}: not known yet; sent a pairing code (sion agent pairing list)",
                        self.word, message.chat
                    );
                    Ok(Admitted::Code(format!(
                        "I do not know you yet. To talk with me, ask the person who runs me to let you in with: sion agent pairing approve {}",
                        crate::pairing::shown(&code)
                    )))
                }
                crate::pairing::Stranger::Quiet => {
                    println!("{} {}: not known yet, ignored", self.word, message.chat);
                    Ok(Admitted::No)
                }
            };
        }
        if options.require_mention && !message.direct {
            // In a group, only what is said to the bot.
            let names = match self.me.get() {
                Some(names) => names,
                None => {
                    let names = self.messenger.me().map_err(CliError::Connection)?;
                    self.me.get_or_init(|| names)
                }
            };
            let named = message
                .mentions
                .iter()
                .any(|m| names.iter().any(|n| n.eq_ignore_ascii_case(m)));
            return Ok(if named { Admitted::Yes } else { Admitted::No });
        }
        Ok(Admitted::Yes)
    }

    fn print(&self, message: &Incoming) {
        // A voice note is shown once it is heard.
        if message.voice.is_some() {
            return;
        }
        let mut text = message.text.clone();
        for note in &message.files {
            let name = note.name.as_deref().unwrap_or("a file");
            text = format!("{text} [{name}]").trim_start().to_string();
        }
        if message.from.is_empty() || message.from == message.chat {
            println!("{} {}: {text}", self.word, message.chat);
        } else {
            println!("{} {} ({}): {text}", self.word, message.chat, message.from);
        }
    }

    /// The files `message` carries, fetched: the pictures, shown to the
    /// agent, and what it is told of each file (where it was put in its
    /// workspace, or why it could not be had).
    fn take_files(&self, message: &Incoming) -> (Vec<sion_agent::Picture>, Vec<String>) {
        let mut fetched = Vec::new();
        let mut unfetched = Vec::new();
        for note in &message.files {
            match self.messenger.fetch_file(note) {
                Ok(file) => fetched.push(file),
                Err(why) => {
                    println!("  (a file not fetched: {why})");
                    unfetched.push(format!(
                        "(A file was sent that could not be fetched: {why}.)"
                    ));
                }
            }
        }
        let workspace = self.profile_for(message).workspace.clone();
        let id = if message.id.is_empty() {
            crate::runtime::new_id()
        } else {
            message.id.clone()
        };
        let (pictures, mut told) = crate::files::take(&fetched, workspace.as_deref(), &id);
        told.extend(unfetched);
        (pictures, told)
    }

    /// Sends the files the agent shared, in answer to `message`: whether
    /// all of them went.
    fn send_files(&self, message: &Incoming, files: &[crate::files::File]) -> bool {
        let mut all = true;
        let mut unsent = Vec::new();
        for file in files {
            match self.delivery.file(self.messenger, message, file) {
                Ok(true) => println!(
                    "to {} {}: [{}, {} bytes]",
                    self.word,
                    message.chat,
                    file.name,
                    file.bytes.len()
                ),
                Ok(false) => unsent.push(file.name.clone()),
                Err(why) => {
                    println!("  (not sent, {}: {why})", file.name);
                    unsent.push(file.name.clone());
                    all = false;
                }
            }
        }
        if !unsent.is_empty() {
            let names = unsent.join(", ");
            let why = if all {
                format!("{} does not take files from me", self.messenger.name())
            } else {
                "they could not be sent".to_string()
            };
            self.say(message, &format!("(I shared {names} with you, but {why}.)"));
        }
        all
    }

    /// Whether the writer of `message` may decide what a session waits on,
    /// and choose its model.
    fn may_decide(&self, message: &Incoming) -> bool {
        self.options.approvers.is_empty() || self.options.approvers.contains(&message.from)
    }

    /// Who the writer of `message` is here, and what they may do.
    fn whoami(&self, message: &Incoming) -> String {
        let who = if message.from.is_empty() {
            "someone the messenger does not name"
        } else {
            &message.from
        };
        let place = if message.thread.is_empty() {
            format!("{} {}", self.word, message.chat)
        } else {
            format!("{} {}, thread {}", self.word, message.chat, message.thread)
        };
        let may = if self.may_decide(message) {
            "talk with it, approve or decline what it waits on, and choose its model".to_string()
        } else {
            format!(
                "talk with it; only {} may approve or decline what it waits on, or choose its model",
                self.options.approvers.join(", ")
            )
        };
        format!(
            "You are {who} on {} in {place}, with {}. You may {may}.",
            self.messenger.name(),
            self.profile_for(message).name
        )
    }

    /// Marks `message`, if messages are marked and the messenger names it.
    fn mark(&self, asides: &Sender<Aside>, message: &Incoming, ack: Ack) {
        if self.options.acknowledge && !message.id.is_empty() {
            let _ = asides.send(Aside::Mark(message.clone(), ack));
        }
    }

    fn do_asides(&self, inbox: Receiver<Aside>) {
        for aside in inbox {
            match aside {
                Aside::Mark(message, ack) => {
                    if let Err(why) = self.messenger.acknowledge(&message, ack) {
                        if !self.marking_failed.swap(true, Ordering::SeqCst) {
                            println!("  (messages are not marked: {why})");
                        }
                    }
                }
                Aside::Say(message, text) => self.say(&message, &text),
                Aside::Flush => {
                    self.delivery.flush(self.messenger);
                }
            }
        }
    }

    /// Says `text` in answer to `message`.
    fn say(&self, message: &Incoming, text: &str) {
        println!("to {} {}: {text}", self.word, message.chat);
        match self.delivery.reply(self.messenger, message, text) {
            Sent::Delivered => {}
            Sent::Held(why) => println!("  (kept, to send once it can be: {why})"),
            Sent::Refused(why) => println!("  (not sent: {why})"),
        }
    }

    /// What `message` says, a voice note heard: what it says goes to the
    /// agent as what its sender said, marked as heard.
    fn hear(&self, message: &Incoming) -> Result<String, String> {
        let Some(note) = &message.voice else {
            return Ok(message.text.clone());
        };
        let heard = match &self.options.hearing {
            None => Err("voice notes are not heard here: write instead".to_string()),
            Some(hearing) => self
                .messenger
                .fetch_voice(note)
                .and_then(|audio| hearing.transcribe(&audio)),
        }?;
        println!("{} {}: (voice note) {heard}", self.word, message.chat);
        Ok(if message.text.is_empty() {
            format!("(voice note) {heard}")
        } else {
            format!("{}\n(voice note) {heard}", message.text)
        })
    }

    /// Answers one turn of conversation `key`: the agent at work shows in
    /// the chat, what the chat says meanwhile goes to it or waits, and the
    /// reply is sent.
    fn answer(
        &self,
        key: String,
        turn: Vec<Waiting>,
        live: &Live,
        notice: bool,
        asides: &Sender<Aside>,
    ) -> Finished {
        let unheard = || lock(&live.said).drain(..).collect::<Vec<_>>();
        let mut answered: Vec<u64> = turn.iter().map(|w| w.seq).collect();
        let Some(last) = turn.last().map(|w| w.message.clone()) else {
            return Finished {
                key,
                answered,
                unheard: unheard(),
            };
        };
        if notice {
            self.say(&last, RESTARTED);
        }
        if let [Waiting {
            canned: Some(text),
            private,
            ..
        }] = turn.as_slice()
        {
            if *private {
                if let Sent::Held(why) | Sent::Refused(why) =
                    self.delivery.reply(self.messenger, &last, text)
                {
                    println!("  (not sent yet: {why})");
                }
            } else {
                self.say(&last, text);
            }
            return Finished {
                key,
                answered,
                unheard: unheard(),
            };
        }
        let mut heard: Vec<(&Incoming, String)> = Vec::new();
        let mut pictures = Vec::new();
        // The voice notes that could not be heard, answered so already.
        let mut not_heard = BTreeSet::new();
        for waiting in &turn {
            match self.hear(&waiting.message) {
                Ok(text) => {
                    let (shown, told) = self.take_files(&waiting.message);
                    pictures.extend(shown);
                    let text = std::iter::once(text)
                        .chain(told)
                        .filter(|t| !t.is_empty())
                        .collect::<Vec<_>>()
                        .join("\n");
                    heard.push((&waiting.message, text))
                }
                Err(why) => {
                    println!(
                        "{} {}: a voice note not heard: {why}",
                        self.word, waiting.message.chat
                    );
                    self.say(&waiting.message, &format!("I could not hear that: {why}"));
                    self.mark(asides, &waiting.message, Ack::Failed);
                    not_heard.insert(waiting.seq);
                }
            }
        }
        if heard.is_empty() {
            return Finished {
                key,
                answered,
                unheard: unheard(),
            };
        }
        let text = together(&heard);
        let options = self.options;
        let aloud = turn.iter().any(|w| w.message.voice.is_some()) && options.speaking.is_some();
        // The chat sees the agent at work: typing, and the reply as it is
        // written (not one it will hear).
        let showing = crate::showing::Showing::start(
            (!options.no_typing)
                .then(|| self.messenger.typist(&last, aloud))
                .flatten(),
            (!aloud && options.stream != Streaming::Off)
                .then(|| self.messenger.drafting(&last))
                .flatten(),
            self.messenger.room(),
            options.stream == Streaming::Progress,
        );
        let watch = |seen: &sion_agent::Seen| showing.seen(seen);
        let stop =
            || live.stopped.load(Ordering::SeqCst) || live.interrupted.load(Ordering::SeqCst);
        // What the chat says meanwhile, as one of its writers said it.
        let steer = || -> Vec<String> {
            let said: Vec<Waiting> = lock(&live.said).drain(..).collect();
            let texts = said
                .iter()
                .map(|w| {
                    println!("  (told to the agent as it works)");
                    if w.message.from.is_empty() || w.message.from == last.from {
                        w.message.text.clone()
                    } else {
                        format!("{}: {}", w.message.from, w.message.text)
                    }
                })
                .collect();
            lock(&live.told).extend(said);
            texts
        };
        let steering = options.busy == Busy::Steer;
        // Who decides, if the turn decides: as the messenger names them.
        let who = format!("{}:{}", self.messenger.name(), last.from);
        let hooks = Hooks {
            watch: Some(&watch),
            stop: Some(&stop),
            steer: steering.then_some(&steer as &(dyn Fn() -> Vec<String> + Sync)),
            who: Some(&who),
            outside: false,
            confidential: false,
            sign: None,
        };
        let answer = crate::channels::converse_with(
            self.ledger.clone(),
            self.profile_for(&last),
            &key,
            &text,
            &pictures,
            hooks,
        );
        let interrupted =
            live.interrupted.load(Ordering::SeqCst) && !live.stopped.load(Ordering::SeqCst);
        let mut shared = Vec::new();
        let (reply, ok) = match answer {
            Ok(answer) => {
                let mut out = std::io::stdout().lock();
                for line in &answer.lines {
                    let _ = std::io::Write::write_all(&mut out, format!("  {line}\n").as_bytes());
                }
                drop(out);
                shared = answer.files;
                // Interrupted, the reply is what it said so far: what was
                // said next is the next turn.
                if interrupted && answer.stopped {
                    if answer.said.trim().is_empty() {
                        (INTERRUPTED.to_string(), true)
                    } else {
                        (answer.said, true)
                    }
                } else {
                    (answer.reply, true)
                }
            }
            Err(e) => (format!("That did not work: {e}"), false),
        };
        // A voice note is answered aloud where the messenger sends voice
        // notes, and in text otherwise.
        let spoken = match (aloud, &options.speaking) {
            (true, Some(speaking)) => match speaking
                .speak(&reply)
                .and_then(|audio| self.messenger.send_voice(&last.chat, &audio, &reply))
            {
                Ok(sent) => sent,
                Err(why) => {
                    println!("  (not spoken: {why})");
                    false
                }
            },
            _ => false,
        };
        // The draft becomes the reply; what it did not deliver, or all of
        // it if nothing was drafted, is sent, part by part.
        let left = showing.end(&reply);
        let mut refused = false;
        if spoken {
            println!("to {} {} (voice note): {reply}", self.word, last.chat);
        } else {
            println!("to {} {}: {reply}", self.word, last.chat);
            for part in left {
                // An interruption that nothing showed needs no word.
                if part.trim().is_empty() || (interrupted && part == INTERRUPTED) {
                    continue;
                }
                match self.delivery.reply(self.messenger, &last, &part) {
                    Sent::Delivered => {}
                    Sent::Held(why) => println!("  (kept, to send once it can be: {why})"),
                    Sent::Refused(why) => {
                        println!("  (not sent: {why})");
                        refused = true;
                    }
                }
            }
        }
        // What it shared goes after what it said.
        if !shared.is_empty() && !self.send_files(&last, &shared) {
            refused = true;
        }
        let told: Vec<Waiting> = lock(&live.told).drain(..).collect();
        let ack = if ok && !refused {
            Ack::Done
        } else {
            Ack::Failed
        };
        for waiting in turn.iter().chain(&told) {
            if !not_heard.contains(&waiting.seq) {
                self.mark(asides, &waiting.message, ack);
            }
        }
        answered.extend(told.iter().map(|w| w.seq));
        Finished {
            key,
            answered,
            unheard: unheard(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(chat: &str, from: &str, text: &str) -> Incoming {
        Incoming {
            id: String::new(),
            thread: String::new(),
            chat: chat.into(),
            from: from.into(),
            text: text.into(),
            voice: None,
            direct: false,
            mentions: Vec::new(),
            files: Vec::new(),
        }
    }

    /// A message waiting, which came in the burst `burst`.
    fn waiting(seq: u64, text: &str, queued: bool, burst: u64) -> Waiting {
        Waiting {
            seq,
            message: message("c1", "ann", text),
            queued,
            burst,
            canned: None,
            private: false,
        }
    }

    fn texts(turn: &[Waiting]) -> Vec<&str> {
        turn.iter().map(|w| w.message.text.as_str()).collect()
    }

    #[test]
    fn a_breaker_opens_after_failures_in_a_row_and_longer_each_time() {
        let start = Instant::now();
        let mut breaker = Breaker::default();
        for _ in 1..TRIP {
            assert!(!breaker.failed(start));
            assert!(breaker.allows(start));
        }
        breaker.sent();
        // A send between them: the count starts again.
        for _ in 1..TRIP {
            assert!(!breaker.failed(start));
        }
        assert!(breaker.failed(start), "the fifth in a row opens it");
        assert!(!breaker.allows(start + OPEN_FOR - Duration::from_secs(1)));
        assert_eq!(breaker.open_until(start), Some(start + OPEN_FOR));
        // Once the time is up, one try: it fails, and the breaker opens
        // again, for twice as long.
        let later = start + OPEN_FOR;
        assert!(breaker.allows(later));
        assert!(breaker.failed(later));
        assert_eq!(breaker.open_until(later), Some(later + OPEN_FOR * 2));
        // Never longer than the most.
        let mut at = later;
        for _ in 0..10 {
            at += OPEN_AT_MOST;
            breaker.failed(at);
        }
        assert_eq!(breaker.open_until(at), Some(at + OPEN_AT_MOST));
        // A send that goes through closes it.
        breaker.sent();
        assert_eq!(breaker, Breaker::default());
    }

    #[test]
    fn only_a_refusal_of_the_message_itself_is_not_tried_again() {
        assert!(lasting(
            "HTTP 400: {\"ok\":false,\"description\":\"Bad Request: chat not found\"}"
        ));
        assert!(lasting("HTTP 403: Forbidden"));
        assert!(lasting("Slack refused chat.postMessage: channel_not_found"));
        assert!(lasting("refused: the address is private"));
        assert!(!lasting("HTTP 429: Too Many Requests"));
        assert!(!lasting("HTTP 408: request timeout"));
        assert!(!lasting("HTTP 502: Bad Gateway"));
        assert!(!lasting("Slack refused chat.postMessage: ratelimited"));
        assert!(!lasting("Slack refused chat.postMessage: internal_error"));
        assert!(!lasting(
            "error trying to connect: Connection refused (os error 111)"
        ));
        assert!(!lasting("the connection timed out"));
    }

    #[test]
    fn a_burst_is_one_turn_and_commands_are_turns_of_their_own() {
        // A burst, then a command, then what came after: the burst
        // together, each of the rest alone.
        let mut chat: VecDeque<Waiting> = [
            waiting(1, "Hi", false, 1),
            waiting(2, "how much is left?", false, 1),
            waiting(3, "/status", false, 3),
            waiting(4, "thanks", false, 4),
            waiting(5, "bye", false, 5),
        ]
        .into();
        assert_eq!(
            texts(&next_turn(&mut chat, Busy::Queue)),
            ["Hi", "how much is left?"]
        );
        assert_eq!(texts(&next_turn(&mut chat, Busy::Queue)), ["/status"]);
        assert_eq!(texts(&next_turn(&mut chat, Busy::Queue)), ["thanks"]);
        assert_eq!(texts(&next_turn(&mut chat, Busy::Queue)), ["bye"]);
        assert!(next_turn(&mut chat, Busy::Queue).is_empty());

        // What came while the agent worked: one at a time when queued,
        // together when collected, or after an interruption.
        let meanwhile = || -> VecDeque<Waiting> {
            [waiting(6, "one", true, 6), waiting(7, "two", true, 7)].into()
        };
        let mut queued = meanwhile();
        assert_eq!(texts(&next_turn(&mut queued, Busy::Queue)), ["one"]);
        assert_eq!(texts(&next_turn(&mut queued, Busy::Queue)), ["two"]);
        for busy in [Busy::Collect, Busy::Interrupt] {
            let mut collected = meanwhile();
            assert_eq!(texts(&next_turn(&mut collected, busy)), ["one", "two"]);
        }

        // A reply known in advance is a turn of its own.
        let mut canned: VecDeque<Waiting> = [
            waiting(8, "Pay Bob", false, 8),
            Waiting {
                canned: Some("Only U9 may approve or decline here.".into()),
                ..waiting(9, "/approve", false, 8)
            },
        ]
        .into();
        assert_eq!(texts(&next_turn(&mut canned, Busy::Steer)), ["Pay Bob"]);
        assert_eq!(texts(&next_turn(&mut canned, Busy::Steer)), ["/approve"]);

        // A turn takes no more than a message's room.
        let long = "x".repeat(MAX_MESSAGE / 3);
        let mut big: VecDeque<Waiting> =
            [waiting(10, &long, false, 10), waiting(11, &long, false, 10)].into();
        assert_eq!(next_turn(&mut big, Busy::Collect).len(), 1);
    }

    #[test]
    fn a_turn_of_many_writers_says_who_wrote_what() {
        let ann = message("c1", "ann", "");
        let bo = message("c1", "bo", "");
        assert_eq!(
            together(&[(&ann, "Hi".into()), (&ann, "Pay Bob".into())]),
            "Hi\nPay Bob"
        );
        assert_eq!(
            together(&[(&ann, "Hi".into()), (&bo, "Wait".into())]),
            "ann: Hi\nbo: Wait"
        );
    }

    #[test]
    fn each_thread_is_a_conversation_of_its_own() {
        let mut in_thread = message("C1", "U1", "Hi");
        assert_eq!(conversation("slack", "lead", &in_thread), "slack:lead:C1");
        in_thread.thread = "1.0".into();
        assert_eq!(
            conversation("slack", "lead", &in_thread),
            "slack:lead:C1:1.0"
        );
    }
}
