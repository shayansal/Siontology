//! What a chat is shown while the agent works on a message: that it is
//! typing, and its reply as it is written.
//!
//! A messenger that can show a chat a typing indicator gives a [`Typist`]
//! for the message it is answering, and one that can show a reply as it is
//! written gives a [`Drafting`]: how to open a [`Draft`] (its platform's
//! own streaming, or a message it edits), how often to update it, and how
//! many updates the platform takes. The bridge watches the session's run
//! ([`sion_agent::Seen`]) and [`Showing`] does the rest on a thread of its
//! own, so that neither the run nor the messenger waits on the other:
//!
//! - the typing indicator shows as soon as the message is taken, and is
//!   shown again before it lapses for as long as the agent works, except
//!   while the reply itself is being written where the chat sees it;
//! - the reply shows as the model writes it, each answer of the run a
//!   paragraph, with the tool the agent runs while it runs it; updates come
//!   no oftener than the messenger takes them, the latest text each time;
//! - a reply longer than one message holds is split where it reads best
//!   ([`pieces`]), each part finished as it fills;
//! - when the run ends, the indicator stops and the draft becomes the reply
//!   the bridge would have sent without it: the same text, which the chat
//!   keeps. What a draft could not deliver, the bridge sends as any reply.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use sion_agent::{Seen, Stop};

use crate::runtime::Spoken;

/// What shows a chat that the agent is at work on its message: a typing
/// indicator ([`crate::messengers::Messenger::typist`]).
pub trait Typist: Send {
    /// Shows it, again if it shows already; how long it lasts unless shown
    /// again.
    fn show(&mut self) -> Result<Duration, String>;
    /// Stops it, where the platform lets a bot; elsewhere it ends when the
    /// reply is sent, or lapses.
    fn stop(&mut self) {}
    /// Whether it may show while a draft of the reply shows: not where the
    /// draft is made of the same signals (a Teams stream).
    fn beside_drafts(&self) -> bool {
        true
    }
}

/// A reply shown as it is written: one message of it, on a platform's own
/// streaming or a message edited as the text grows.
pub trait Draft: Send {
    /// Shows `text`, all of this message of the reply so far, and what the
    /// agent is doing besides writing it (`status`: a tool it runs), if
    /// anything. The first call shows it; each later one replaces it.
    fn show(&mut self, text: &str, status: Option<&str>) -> Result<(), String>;
    /// Ends it as `text`: the message the chat keeps.
    fn finish(&mut self, text: &str) -> Result<(), String>;
}

/// How a messenger shows a reply as it is written
/// ([`crate::messengers::Messenger::drafting`]).
pub struct Drafting {
    /// Opens the draft of the next message of the reply. Nothing is shown
    /// before its first [`Draft::show`].
    pub open: Box<dyn FnMut() -> Box<dyn Draft> + Send>,
    /// The least time between two updates.
    pub every: Duration,
    /// The most updates shown before the end, if the platform counts them
    /// (an edit on Matrix is an event of its own).
    pub most: Option<usize>,
    /// For a draft that lapses unless it is shown again (Telegram's last
    /// 30 seconds): how long it may go without an update.
    pub keep: Option<Duration>,
    /// The fewest characters of text the first update shows. A message
    /// edited as it grows notifies the chat when it is first sent, so it
    /// waits for something worth reading; a platform's own stream (0)
    /// shows at once, and shows what the agent does before it writes.
    pub first: usize,
}

impl std::fmt::Debug for Drafting {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Drafting")
            .field("every", &self.every)
            .field("most", &self.most)
            .field("keep", &self.keep)
            .field("first", &self.first)
            .finish_non_exhaustive()
    }
}

/// The fewest characters a part of a reply is cut to.
const MIN_ROOM: usize = 40;

/// What closes a code block a part ends inside.
const FENCE: &str = "```";

/// `text` in parts of at most `room` characters each, split where it reads
/// best: the last paragraph break in a part's room, or else its last line
/// break, sentence end or space (each only past a part's first third, so
/// that no part is much shorter than it could be), or else at its room. A
/// part that ends inside a code block closes it, and the next opens it
/// again with the same fence line. Empty text is one empty part.
pub fn pieces(text: &str, room: usize) -> Vec<String> {
    let room = room.max(MIN_ROOM);
    let mut parts = Vec::new();
    let mut rest = text.trim().to_string();
    let mut reopen: Option<String> = None;
    loop {
        let (body, opened) = match reopen.take() {
            Some(fence) => (format!("{fence}\n{rest}"), fence.len() + 1),
            None => (rest.clone(), 0),
        };
        if body.chars().count() <= room {
            parts.push(body);
            break;
        }
        // Room for the fence that closes a block the part ends inside.
        let limit = byte_at(&body, room - FENCE.len() - 1);
        let mut at = split_point(&body[..limit]);
        if at <= opened {
            at = limit;
        }
        let (head, tail) = body.split_at(at);
        let mut head = head.trim_end().to_string();
        let inside = open_fence(&head);
        if inside.is_some() {
            head.push('\n');
            head.push_str(FENCE);
        }
        parts.push(head);
        rest = match &inside {
            // Inside a block, only the line break goes: indentation stays.
            Some(_) => tail.trim_start_matches(['\n', '\r']).to_string(),
            None => tail.trim_start().to_string(),
        };
        reopen = inside;
        if rest.trim().is_empty() {
            break;
        }
    }
    parts
}

/// The byte offset of the character at `chars` in `text`, or its length.
fn byte_at(text: &str, chars: usize) -> usize {
    text.char_indices()
        .nth(chars)
        .map(|(at, _)| at)
        .unwrap_or(text.len())
}

/// Where in `window` (the room of a part) to split: before its last
/// paragraph break past half of it, or else its last line break, sentence
/// end or space past a third of it, or else at its end.
fn split_point(window: &str) -> usize {
    let len = window.len();
    let past = |at: usize, part: usize| at > 0 && at >= len / part;
    if let Some(at) = window.rfind("\n\n").filter(|&at| past(at, 2)) {
        return at;
    }
    if let Some(at) = window.rfind('\n').filter(|&at| past(at, 3)) {
        return at;
    }
    let sentence = [". ", "! ", "? ", "; ", "。", "！", "？"]
        .iter()
        .filter_map(|end| window.rfind(end).map(|at| at + end.trim_end().len()))
        .max();
    if let Some(at) = sentence.filter(|&at| past(at, 3)) {
        return at;
    }
    if let Some(at) = window
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_whitespace())
        .map(|(at, _)| at)
        .filter(|&at| past(at, 3))
    {
        return at;
    }
    len
}

/// The fence line of the code block `text` ends inside, if it does.
fn open_fence(text: &str) -> Option<String> {
    let mut open: Option<String> = None;
    for line in text.lines() {
        if line.trim_start().starts_with(FENCE) {
            open = match open {
                Some(_) => None,
                None => Some(line.trim().to_string()),
            };
        }
    }
    open
}

/// `text` with what the agent is doing, `status`, after it: the text a
/// draft that is replaced whole shows.
pub fn with_status(text: &str, status: Option<&str>) -> String {
    match status {
        None => text.to_string(),
        Some(status) if text.trim().is_empty() => format!("… {status}"),
        Some(status) => format!("{text}\n\n… {status}"),
    }
}

/// How long before a typing indicator lapses it is shown again.
const MARGIN: Duration = Duration::from_millis(1_000);

/// The least time between two typing indicators.
const MIN_REFRESH: Duration = Duration::from_millis(1_000);

/// For how long after the reply last grew where the chat sees it no typing
/// indicator is needed: the reply shows the agent at work.
const WRITING: Duration = Duration::from_secs(3);

/// What the thread is told.
enum Told {
    Seen(Seen),
    End(String),
}

/// What a bridge shows a chat while the agent works on a message, from a
/// thread of its own ([`Showing::start`]): told what the run does
/// ([`Showing::seen`]), and ended with the reply ([`Showing::end`]).
pub struct Showing {
    tell: mpsc::Sender<Told>,
    worker: Option<JoinHandle<Vec<String>>>,
    room: usize,
}

impl std::fmt::Debug for Showing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Showing")
            .field("room", &self.room)
            .finish_non_exhaustive()
    }
}

impl Showing {
    /// Starts showing a chat the agent at work: with `typist` its typing
    /// indicator, at once; with `drafting`, the reply as it is written, in
    /// parts of at most `room` characters, and with `progress` the tool the
    /// agent runs, even before it has written anything.
    pub fn start(
        typist: Option<Box<dyn Typist>>,
        drafting: Option<Drafting>,
        room: usize,
        progress: bool,
    ) -> Showing {
        let (tell, told) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            Worker {
                typist,
                drafting,
                room,
                progress,
                ..Worker::default()
            }
            .work(told)
        });
        Showing {
            tell,
            worker: Some(worker),
            room,
        }
    }

    /// Tells it what the run did.
    pub fn seen(&self, seen: &Seen) {
        let _ = self.tell.send(Told::Seen(seen.clone()));
    }

    /// Ends it with `reply`: the indicator stops and the draft becomes the
    /// reply. Returns the parts of the reply the chat does not have yet, to
    /// send as any reply: all of it, when nothing was drafted.
    pub fn end(mut self, reply: &str) -> Vec<String> {
        let _ = self.tell.send(Told::End(reply.to_string()));
        match self.worker.take().map(JoinHandle::join) {
            Some(Ok(left)) => left,
            _ => pieces(reply, self.room),
        }
    }
}

/// The thread's state.
#[derive(Default)]
struct Worker {
    typist: Option<Box<dyn Typist>>,
    drafting: Option<Drafting>,
    room: usize,
    /// Whether the draft shows the tool the agent runs.
    progress: bool,
    /// What the run said so far.
    spoken: Spoken,
    /// What the agent does besides writing, if anything.
    status: Option<String>,
    /// When the typing indicator is due again (none: not shown any more).
    typing_due: Option<Instant>,
    /// Whether the draft is behind what the run said.
    behind: bool,
    /// When the draft was last updated.
    updated: Option<Instant>,
    /// When the reply last grew where the chat sees it.
    wrote: Option<Instant>,
    /// Updates shown.
    updates: usize,
    /// Parts of the reply finished.
    settled: usize,
    /// The draft of the part being written, once opened.
    current: Option<Box<dyn Draft>>,
    /// What the draft shows: its text and status.
    shown: (String, Option<String>),
    /// Whether the draft failed: it is not updated again.
    broken: bool,
}

impl Worker {
    fn work(mut self, told: Receiver<Told>) -> Vec<String> {
        self.typing_due = self.typist.as_ref().map(|_| Instant::now());
        loop {
            let now = Instant::now();
            self.type_if_due(now);
            self.draft_if_due(now);
            let now = Instant::now();
            let wait = [self.typing_due, self.draft_due(now)]
                .into_iter()
                .flatten()
                .min()
                .map(|at| at.saturating_duration_since(now));
            let next = match wait {
                Some(wait) => told.recv_timeout(wait),
                None => told.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            match next {
                Ok(Told::Seen(seen)) => self.apply(seen),
                Ok(Told::End(reply)) => {
                    self.stop_typing();
                    return self.finish(&reply);
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    self.stop_typing();
                    return Vec::new();
                }
            }
        }
    }

    fn apply(&mut self, seen: Seen) {
        match seen {
            Seen::Began(_) | Seen::Asking | Seen::Checking => {}
            Seen::Pausing(seconds) if self.progress => {
                self.status = Some(format!("the models are busy: asking again in {seconds} s"));
                self.behind = true;
            }
            Seen::Pausing(_) => {}
            Seen::Planned(plan) if self.progress => {
                self.status = Some(format!("plan: {plan}"));
                self.behind = true;
            }
            Seen::Planned(_) => {}
            Seen::Text(text) => {
                self.spoken.add(&text);
                self.status = None;
                self.behind = true;
            }
            Seen::Calling(tool) | Seen::Running(tool) if self.progress => {
                self.status = Some(tool);
                self.behind = true;
            }
            Seen::Waiting(tool) if self.progress => {
                self.status = Some(format!("{tool}: waiting for your approval"));
                self.behind = true;
            }
            Seen::Calling(_) | Seen::Running(_) | Seen::Waiting(_) => {}
            Seen::Answered(stop) => self.spoken.end(stop == Stop::MaxTokens),
            Seen::Dropped => {
                self.spoken.drop_current();
                self.behind = true;
            }
        }
    }

    fn type_if_due(&mut self, now: Instant) {
        let (Some(typist), Some(due)) = (self.typist.as_mut(), self.typing_due) else {
            return;
        };
        if now < due {
            return;
        }
        if self.updates > 0 && !typist.beside_drafts() {
            self.typing_due = None;
            return;
        }
        // The reply growing where the chat sees it shows the agent at work.
        if let Some(wrote) = self.wrote.filter(|at| now.duration_since(*at) < WRITING) {
            self.typing_due = Some(wrote + WRITING);
            return;
        }
        self.typing_due = match typist.show() {
            Ok(lasts) => Some(now + lasts.saturating_sub(MARGIN).max(MIN_REFRESH)),
            Err(why) => {
                println!("  (no typing indicator: {why})");
                None
            }
        };
    }

    fn stop_typing(&mut self) {
        if let Some(typist) = self.typist.as_mut() {
            if self.typing_due.is_some() || self.updates > 0 {
                typist.stop();
            }
        }
        self.typing_due = None;
    }

    /// Whether the draft may take another update.
    fn may_update(&self) -> bool {
        self.behind
            && !self
                .drafting
                .as_ref()
                .is_some_and(|d| d.most.is_some_and(|most| self.updates >= most))
    }

    /// When the draft is due an update, as of `now`: if it is behind and
    /// may be updated, or if it lapses unless shown again.
    fn draft_due(&self, now: Instant) -> Option<Instant> {
        let drafting = self.drafting.as_ref()?;
        if self.broken {
            return None;
        }
        if self.may_update() {
            return Some(self.updated.map_or(now, |at| at + drafting.every));
        }
        let keep = drafting.keep?;
        let shown = self.current.is_some() && !self.shown.0.trim().is_empty();
        shown.then(|| self.updated.map(|at| at + keep)).flatten()
    }

    fn draft_if_due(&mut self, now: Instant) {
        if !self.draft_due(now).is_some_and(|due| due <= now) {
            return;
        }
        if !self.may_update() {
            // Shown again as it is, before it lapses.
            let (text, status) = self.shown.clone();
            if let Some(draft) = self.current.as_mut() {
                if let Err(why) = draft.show(&text, status.as_deref()) {
                    println!("  (the reply is not shown as it is written: {why})");
                    self.broken = true;
                }
            }
            self.updated = Some(now);
            return;
        }
        self.behind = false;
        let text = self.spoken.text();
        let Some(drafting) = self.drafting.as_mut() else {
            return;
        };
        if self.current.is_none() && self.settled == 0 {
            // Nothing shown yet: a status only where the platform's own
            // stream shows it, text only once there is enough of it.
            let enough = text.chars().count() >= drafting.first.max(1);
            if !enough && !(drafting.first == 0 && self.status.is_some()) {
                return;
            }
        }
        let parts = pieces(&text, self.room);
        let result = (|| {
            // The parts that filled are finished, each in its own message.
            while self.settled + 1 < parts.len() {
                let mut draft = self.current.take().unwrap_or_else(|| (drafting.open)());
                draft.finish(&parts[self.settled])?;
                self.settled += 1;
                self.shown = Default::default();
            }
            let part = &parts[self.settled.min(parts.len() - 1)];
            let shown = (part.clone(), self.status.clone());
            // A dropped answer leaves the draft as it was until more comes.
            if shown == self.shown || with_status(part, self.status.as_deref()).trim().is_empty() {
                return Ok(());
            }
            let draft = self.current.get_or_insert_with(|| (drafting.open)());
            draft.show(part, self.status.as_deref())?;
            self.updates += 1;
            self.updated = Some(now);
            if !part.trim().is_empty() {
                self.wrote = Some(now);
            }
            self.shown = shown;
            Ok::<(), String>(())
        })();
        if let Err(why) = result {
            println!("  (the reply is not shown as it is written: {why})");
            self.broken = true;
        }
    }

    /// The draft becomes `reply`: the parts it finished, then the part it
    /// shows; the parts left, to send as any reply.
    fn finish(&mut self, reply: &str) -> Vec<String> {
        let parts = pieces(reply, self.room);
        if self.current.is_none() && self.settled == 0 {
            return parts;
        }
        let mut left: Vec<String> = parts.into_iter().skip(self.settled).collect();
        if left.is_empty() {
            return left;
        }
        if let Some(mut draft) = self.current.take() {
            let first = left.remove(0);
            if let Err(why) = draft.finish(&first) {
                println!("  (the draft did not become the reply: {why})");
                left.insert(0, first);
            }
        }
        left
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn short_text_is_one_part_and_long_text_splits_where_it_reads_best() {
        assert_eq!(pieces("  Hello.  ", 100), vec!["Hello."]);
        assert_eq!(pieces("", 100), vec![""]);
        let paragraphs = format!("{}\n\n{}", "a".repeat(30), "b".repeat(30));
        assert_eq!(
            pieces(&paragraphs, 45),
            vec!["a".repeat(30), "b".repeat(30)]
        );
        let sentences = "The first sentence is here. The second one follows it.";
        assert_eq!(
            pieces(sentences, 40),
            vec!["The first sentence is here.", "The second one follows it."]
        );
        let words = "word ".repeat(30);
        for part in pieces(&words, 40) {
            assert!(part.chars().count() <= 40, "{part:?}");
            assert!(!part.starts_with(' ') && !part.ends_with(' '), "{part:?}");
        }
        // No space anywhere: cut at the room.
        let solid = "x".repeat(100);
        let parts = pieces(&solid, 40);
        assert_eq!(parts.concat(), solid);
        assert!(parts.iter().all(|p| p.chars().count() <= 40));
        // Characters, not bytes.
        let wide = "é".repeat(90);
        assert!(pieces(&wide, 40).iter().all(|p| p.chars().count() <= 40));
    }

    #[test]
    fn a_code_block_split_is_closed_and_opened_again() {
        let code: String = (0..12).map(|i| format!("    let x{i} = {i};\n")).collect();
        let text = format!("Here:\n```rust\n{code}```\nDone.");
        let parts = pieces(&text, 80);
        assert!(parts.len() > 2, "{parts:?}");
        for part in &parts {
            assert!(part.chars().count() <= 80, "{part:?}");
            assert_eq!(part.matches("```").count() % 2, 0, "{part:?}");
        }
        assert!(parts[1].starts_with("```rust\n    let"), "{:?}", parts[1]);
        let joined = parts.join("\n");
        assert!(joined.contains("let x11 = 11;") && joined.ends_with("Done."));
    }

    #[test]
    fn status_follows_the_text() {
        assert_eq!(with_status("Hi", None), "Hi");
        assert_eq!(with_status("", Some("ledger_balance")), "… ledger_balance");
        assert_eq!(
            with_status("Checking.", Some("ledger_balance")),
            "Checking.\n\n… ledger_balance"
        );
    }

    /// What the fakes were asked to do, in order.
    type Log = Arc<Mutex<Vec<String>>>;

    struct FakeTypist(Log);

    impl Typist for FakeTypist {
        fn show(&mut self) -> Result<Duration, String> {
            self.0.lock().unwrap().push("typing".into());
            Ok(Duration::from_millis(1_500))
        }
        fn stop(&mut self) {
            self.0.lock().unwrap().push("typing stops".into());
        }
    }

    struct FakeDraft(Log, usize);

    impl Draft for FakeDraft {
        fn show(&mut self, text: &str, status: Option<&str>) -> Result<(), String> {
            let mut log = self.0.lock().unwrap();
            log.push(format!("{} shows {:?}", self.1, with_status(text, status)));
            Ok(())
        }
        fn finish(&mut self, text: &str) -> Result<(), String> {
            self.0
                .lock()
                .unwrap()
                .push(format!("{} is {text:?}", self.1));
            Ok(())
        }
    }

    fn drafting(log: &Log) -> Drafting {
        let log = log.clone();
        let mut opened = 0;
        Drafting {
            open: Box::new(move || {
                opened += 1;
                Box::new(FakeDraft(log.clone(), opened))
            }),
            every: Duration::from_millis(50),
            most: None,
            keep: None,
            first: 0,
        }
    }

    /// A run that checks the balance, then says so a word at a time.
    fn run(showing: &Showing) {
        std::thread::sleep(Duration::from_millis(100));
        showing.seen(&Seen::Asking);
        showing.seen(&Seen::Calling("ledger_balance".into()));
        showing.seen(&Seen::Answered(Stop::ToolUse));
        showing.seen(&Seen::Running("ledger_balance".into()));
        std::thread::sleep(Duration::from_millis(100));
        showing.seen(&Seen::Asking);
        for word in ["100 ", "USD ", "left."] {
            showing.seen(&Seen::Text(word.into()));
            std::thread::sleep(Duration::from_millis(80));
        }
        showing.seen(&Seen::Answered(Stop::EndTurn));
    }

    #[test]
    fn typing_shows_at_once_and_the_reply_as_it_is_written() {
        let log: Log = Arc::default();
        let showing = Showing::start(
            Some(Box::new(FakeTypist(log.clone()))),
            Some(drafting(&log)),
            4_000,
            true,
        );
        run(&showing);
        let left = showing.end("100 USD left.");
        assert!(left.is_empty(), "{left:?}");
        let log = log.lock().unwrap().clone();
        assert_eq!(log[0], "typing", "{log:?}");
        // The tool shows while it runs, then the text as it is written.
        assert!(
            log.contains(&"1 shows \"… ledger_balance\"".to_string()),
            "{log:?}"
        );
        assert!(log.contains(&"1 shows \"100\"".to_string()), "{log:?}");
        assert_eq!(log.last().unwrap(), "1 is \"100 USD left.\"", "{log:?}");
        assert!(log.contains(&"typing stops".to_string()), "{log:?}");

        // Text only: the tool does not show.
        let log: Log = Arc::default();
        let showing = Showing::start(None, Some(drafting(&log)), 4_000, false);
        run(&showing);
        assert!(showing.end("100 USD left.").is_empty());
        let log = log.lock().unwrap().clone();
        assert!(!log.iter().any(|l| l.contains('…')), "{log:?}");
        assert_eq!(log.last().unwrap(), "1 is \"100 USD left.\"", "{log:?}");
    }

    #[test]
    fn typing_alone_lasts_as_long_as_the_work() {
        let log: Log = Arc::default();
        let showing = Showing::start(Some(Box::new(FakeTypist(log.clone()))), None, 4_000, true);
        // Shown at once, and again before each lapse (1.5 s, less a second).
        std::thread::sleep(Duration::from_millis(1_700));
        let left = showing.end("Done.");
        assert_eq!(left, vec!["Done."]);
        let log = log.lock().unwrap().clone();
        let typed = log.iter().filter(|l| *l == "typing").count();
        assert!(typed >= 2, "{log:?}");
        assert_eq!(log.last().unwrap(), "typing stops", "{log:?}");
    }

    #[test]
    fn a_long_reply_fills_part_after_part_and_nothing_drafted_is_sent_as_ever() {
        let log: Log = Arc::default();
        let showing = Showing::start(None, Some(drafting(&log)), 50, true);
        let words: Vec<String> = (0..30).map(|i| format!("w{i:02} ")).collect();
        for word in &words {
            showing.seen(&Seen::Text(word.clone()));
            std::thread::sleep(Duration::from_millis(15));
        }
        showing.seen(&Seen::Answered(Stop::EndTurn));
        std::thread::sleep(Duration::from_millis(80));
        let reply = words.concat().trim().to_string();
        let left = showing.end(&reply);
        let parts = pieces(&reply, 50);
        assert!(parts.len() >= 2);
        let log = log.lock().unwrap().clone();
        let finished: Vec<&String> = log.iter().filter(|l| l.contains(" is ")).collect();
        assert_eq!(finished.len() + left.len(), parts.len(), "{log:?} {left:?}");
        assert!(log.iter().any(|l| l.starts_with("1 is ")), "{log:?}");
        assert!(log.iter().any(|l| l.starts_with("2 ")), "{log:?}");

        // A reply nothing was drafted for is the bridge's to send.
        let quiet = Showing::start(None, Some(drafting(&Log::default())), 50, true);
        assert_eq!(
            quiet.end("Only /approve here."),
            vec!["Only /approve here."]
        );
    }

    #[test]
    fn an_answer_dropped_partway_leaves_the_draft() {
        let log: Log = Arc::default();
        let showing = Showing::start(None, Some(drafting(&log)), 4_000, true);
        showing.seen(&Seen::Text("Half an ans".into()));
        std::thread::sleep(Duration::from_millis(80));
        showing.seen(&Seen::Dropped);
        std::thread::sleep(Duration::from_millis(80));
        showing.seen(&Seen::Text("The whole answer.".into()));
        showing.seen(&Seen::Answered(Stop::EndTurn));
        std::thread::sleep(Duration::from_millis(80));
        let left = showing.end("The whole answer.");
        assert!(left.is_empty());
        let log = log.lock().unwrap().clone();
        assert!(
            log.contains(&"1 shows \"Half an ans\"".to_string()),
            "{log:?}"
        );
        assert!(!log.iter().any(|l| l.contains("Half an ansThe")), "{log:?}");
        assert_eq!(log.last().unwrap(), "1 is \"The whole answer.\"");
    }
}
