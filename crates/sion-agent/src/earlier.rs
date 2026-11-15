//! The earlier part of a long conversation, kept when it is let go.
//!
//! A session keeps its conversation within a length its models take
//! ([`crate::session`]): the results of calls long past are cut, and the
//! middle is replaced by notes. What is taken out is not thrown away. It is
//! kept here, whole and in order, in the session's own state, so that:
//!
//! - the agent can look back ([`LOOK_BACK`]): search what was let go by
//!   words, or read a message as it was. Its notes say what was let go and
//!   which names they do not hold ([`not_held`]), so it knows what it would
//!   have to look up rather than guess;
//! - the whole conversation can still be read ([`Earlier::whole`]) by what
//!   checks or learns from a session: a task's checks, a harvest, a
//!   measurement of what the notes lost;
//! - notes a model wrote are checked against what they replace
//!   ([`unsupported`]): a name that is nowhere in it was made up.
//!
//! How a conversation is kept short is a setting ([`Keeping`]), not a
//! constant: what the notes' writer is told and asked, how many messages
//! stay as they are, which results are cut first. It is what an improvement
//! loop changes and measures.
//!
//! None of this makes notes hold what will matter later. It makes what they
//! leave out recoverable, says what that is, and lets the loss be measured.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::model::{Block, Delta, Message, Model, ModelError, Request, Response, Role, ToolSpec};
use crate::tools::search_words;

/// The tool with which the agent looks back at what its conversation let
/// go.
pub const LOOK_BACK: &str = "look_back";

/// The most characters of let-go conversation a session keeps, unless its
/// limits say otherwise: past it, the oldest messages go, and are counted.
pub const EARLIER_CHARS: usize = 3_000_000;

/// The most shortenings a session keeps a record of: the last ones.
pub const SPANS_KEPT: usize = 200;

/// The most looks back a session keeps a record of: the last ones.
pub const LOOKS_KEPT: usize = 100;

/// The most matches a search of what was let go returns.
pub const FOUND: usize = 8;

/// The most characters of a message a look back returns at a time.
pub const READ_CHARS: usize = 12_000;

/// How many of the names notes do not hold are listed after them, unless
/// the session's limits say otherwise.
pub const NAMES_LISTED: usize = 24;

/// The longest name that is listed or checked.
const NAME_CHARS: usize = 96;

/// The most characters mechanical notes keep: past it, their oldest lines
/// go.
pub const MECHANICAL_CHARS: usize = 8_000;

/// What begins the line that says what a conversation let go.
pub const LET_GO: &str = "[Let go to save context:";

/// What begins the line that lists the names notes do not hold.
pub const NOT_HELD: &str = "[Not in these notes:";

/// What begins the line that names what a model's notes made up.
pub const MADE_UP: &str = "[Nowhere in what these notes replace:";

/// One shortening of a conversation: what it let go, and how its notes
/// stood against it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    /// The number of the first message it let go.
    pub from: usize,
    /// The number of the last.
    pub to: usize,
    /// Who wrote the notes: a model's name, or nothing for the mechanical
    /// ones.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub by: String,
    /// The names a model's notes held that are nowhere in what they
    /// replace, after it was asked again: flagged beside the notes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub made_up: Vec<String>,
    /// Whether the notes' writer was asked a second time, its first notes
    /// having named what is nowhere in what they replace.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub asked_again: bool,
    /// How many names of everything let go so far the shortened
    /// conversation then held nowhere: not in its notes, not in what the
    /// runtime keeps beside them, not in the messages kept as they are.
    #[serde(default)]
    pub not_held: usize,
    /// The conversation's characters before it.
    #[serde(default)]
    pub before: usize,
    /// And after.
    #[serde(default)]
    pub after: usize,
}

/// One look back: what was asked, and the messages it gave.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Look {
    /// What was asked: the words searched for, or which message or result
    /// was read.
    pub asked: String,
    /// Whether it read a message or a result by its number or call,
    /// rather than searched.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub read: bool,
    /// The numbers of the messages it returned, or matched.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub found: Vec<usize>,
}

/// What a session's conversation let go, kept as it was.
///
/// Messages are numbered through the whole conversation: the task is 0,
/// and a message keeps its number when it is let go. The runtime's own
/// notes (a summary, and the word that follows it) have none.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Earlier {
    /// The messages let go, as they were, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<Message>,
    /// The results cut from messages still in the conversation, as they
    /// were, by the call each answers.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub cut: BTreeMap<String, String>,
    /// The calls whose results came from outside the agent's trust: read
    /// again, they are content from outside again.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub outside: BTreeSet<String>,
    /// The shortenings, oldest first: the last [`SPANS_KEPT`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spans: Vec<Span>,
    /// How many messages after the task are the runtime's notes, not
    /// conversation.
    #[serde(default)]
    pub notes: usize,
    /// The notes as last written, without what the runtime puts around
    /// them: what the next notes are written from, with the messages since.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub body: String,
    /// How many of the oldest messages let go are no longer kept, the
    /// session's limit having been reached.
    #[serde(default)]
    pub dropped: usize,
    /// What the agent looked back for, oldest first: the last
    /// [`LOOKS_KEPT`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub looks: Vec<Look>,
    /// How many looks back there were in all.
    #[serde(default)]
    pub looked: u64,
    /// Whether the conversation was shortened before what it let go was
    /// kept (a session from before that was kept, or one that keeps
    /// nothing): its whole is then not known.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unkept: bool,
}

/// What a look back gave.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Looked {
    /// What the model is shown.
    pub content: String,
    /// Whether nothing could be given.
    pub is_error: bool,
    /// Whether it holds a result that came from outside the agent's trust.
    pub outside: bool,
}

/// A match of a search of what was let go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    /// The message's number.
    pub number: usize,
    /// What of it matched: who said it, or which call's result it is.
    pub what: String,
    /// The text around the match.
    pub snippet: String,
    /// The call it is the result of, if it is one.
    pub call: Option<String>,
}

/// A part of a message as a search reads it.
struct Part {
    number: usize,
    what: String,
    text: String,
    call: Option<String>,
}

impl Earlier {
    /// Whether it holds nothing at all: a session that never shortened its
    /// conversation.
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
            && self.cut.is_empty()
            && self.outside.is_empty()
            && self.spans.is_empty()
            && self.notes == 0
            && self.body.is_empty()
            && self.dropped == 0
            && self.looks.is_empty()
            && self.looked == 0
            && !self.unkept
    }

    /// Whether there is anything to look back at.
    pub fn holds(&self) -> bool {
        !self.messages.is_empty() || !self.cut.is_empty()
    }

    /// Whether every message let go is still kept, so that the whole
    /// conversation can be read ([`Earlier::whole`]).
    pub fn is_whole(&self) -> bool {
        self.dropped == 0 && !self.unkept
    }

    /// The characters it keeps.
    pub fn chars(&self) -> usize {
        self.messages.iter().map(Message::chars).sum::<usize>()
            + self.cut.values().map(String::len).sum::<usize>()
    }

    /// The number of the last message let go, if any was.
    pub fn last(&self) -> Option<usize> {
        let count = self.dropped + self.messages.len();
        (count > 0).then_some(count)
    }

    /// The number, in the whole conversation, of the message at `index` of
    /// the conversation as it now is (`live`): none for the task's own
    /// place past the first, or for the runtime's notes.
    pub fn number(&self, index: usize) -> Option<usize> {
        match index {
            0 => Some(0),
            index if index <= self.notes => None,
            index => Some(self.dropped + self.messages.len() + index - self.notes),
        }
    }

    /// How many messages the whole conversation has, the task and what was
    /// let go included: one more than the last one's number.
    pub fn total(&self, live: &[Message]) -> usize {
        self.dropped
            + self.messages.len()
            + live
                .len()
                .saturating_sub(self.notes.min(live.len().saturating_sub(1)))
    }

    /// The messages of the whole conversation numbered `number` and above,
    /// as they were: those still kept.
    pub fn since(&self, live: &[Message], number: usize) -> Vec<Message> {
        (number..self.total(live))
            .filter_map(|n| self.message(live, n).ok())
            .collect()
    }

    /// Notes that a result of the call `id` came from outside the agent's
    /// trust.
    pub fn from_outside(&mut self, id: &str) {
        self.outside.insert(id.to_string());
    }

    /// Keeps the result of the call `id` as it was, before it is cut from
    /// the conversation. False, and nothing is kept, when another result
    /// still in the conversation is kept under that id: a model gave two
    /// calls one id, and to cut the second would be to lose the first. The
    /// second then stays as it is.
    pub fn keep_cut(&mut self, id: &str, content: &str) -> bool {
        match self.cut.get(id) {
            Some(kept) if kept != content => false,
            _ => {
                self.cut.insert(id.to_string(), content.to_string());
                true
            }
        }
    }

    /// Takes `messages`, the conversation's next ones in order, as they
    /// were, the results cut from them given back. Past `most` characters
    /// the oldest messages go, and are counted.
    pub fn let_go(&mut self, messages: &[Message], most: usize) {
        for message in messages {
            let restored = self.restored(message);
            for block in &message.content {
                if let Some(id) = cut_result(block) {
                    self.cut.remove(id);
                }
            }
            self.messages.push(restored);
        }
        self.fit(most);
    }

    /// `message` with the results cut from it given back as they were:
    /// the message as it was before anything was cut from it.
    pub fn restored(&self, message: &Message) -> Message {
        let mut whole = message.clone();
        for block in &mut whole.content {
            let Some(was) = cut_result(block).and_then(|id| self.cut.get(id)) else {
                continue;
            };
            if let Block::ToolResult { content, .. } = block {
                content.clone_from(was);
            }
        }
        whole
    }

    /// Lets the oldest messages go for good until what is kept fits `most`
    /// characters.
    fn fit(&mut self, most: usize) {
        let mut chars = self.chars();
        let mut gone = 0;
        while chars > most && gone < self.messages.len() {
            chars -= self.messages[gone].chars();
            gone += 1;
        }
        if gone == 0 {
            return;
        }
        for message in self.messages.drain(..gone) {
            for block in &message.content {
                if let Block::ToolResult { id, .. } = block {
                    self.outside.remove(id);
                }
            }
        }
        self.dropped += gone;
    }

    /// Records a shortening.
    pub fn record(&mut self, span: Span) {
        self.spans.push(span);
        let over = self.spans.len().saturating_sub(SPANS_KEPT);
        self.spans.drain(..over);
    }

    /// The whole conversation, from what is kept here and the conversation
    /// as it now is (`live`): the task, the messages let go, and those
    /// still there with the results cut from them given back. The
    /// runtime's notes are not part of it. It is the whole only while
    /// nothing was dropped ([`Earlier::is_whole`]).
    pub fn whole(&self, live: &[Message]) -> Vec<Message> {
        let mut all = Vec::with_capacity(self.messages.len() + live.len());
        all.extend(live.first().cloned());
        all.extend(self.messages.iter().cloned());
        for message in live.iter().skip(1 + self.notes) {
            all.push(self.restored(message));
        }
        all
    }

    /// Which tool each call was, in what is kept and in `live`.
    fn tools<'a>(&'a self, live: &'a [Message]) -> BTreeMap<&'a str, &'a str> {
        self.messages
            .iter()
            .chain(live)
            .flat_map(Message::tool_uses)
            .map(|(id, name, _)| (id, name))
            .collect()
    }

    /// What a search reads: every part of the messages let go, and the
    /// results cut from the messages of `live`.
    fn parts(&self, live: &[Message]) -> Vec<Part> {
        let tools = self.tools(live);
        let of = |id: &str| format!("the result of {}", tools.get(id).unwrap_or(&"a call"));
        let mut parts = Vec::new();
        for (at, message) in self.messages.iter().enumerate() {
            let number = self.dropped + at + 1;
            for block in &message.content {
                let (what, text, call) = match block {
                    Block::Text { text } if message.role == Role::Assistant => {
                        ("you said".to_string(), text.clone(), None)
                    }
                    Block::Text { text } => ("the person said".to_string(), text.clone(), None),
                    Block::ToolUse { name, input, .. } => (
                        format!("you called {name}"),
                        format!("{name} {input}"),
                        None,
                    ),
                    Block::ToolResult { id, content, .. } => {
                        (of(id), content.clone(), Some(id.clone()))
                    }
                    Block::Image { .. } | Block::Reasoning { .. } => continue,
                };
                parts.push(Part {
                    number,
                    what,
                    text,
                    call,
                });
            }
        }
        for (index, message) in live.iter().enumerate() {
            let Some(number) = self.number(index) else {
                continue;
            };
            for block in &message.content {
                if let Some(id) = cut_result(block) {
                    if let Some(was) = self.cut.get(id) {
                        parts.push(Part {
                            number,
                            what: of(id),
                            text: was.clone(),
                            call: Some(id.to_string()),
                        });
                    }
                }
            }
        }
        parts
    }

    /// The parts of what was let go that best match `query`, the best
    /// first, at most `most`: ranked by BM25 over their words
    /// ([`search_words`]), a part that holds the query as it is written
    /// before any that does not. A part that shares no word with the query
    /// is not returned.
    pub fn search(&self, live: &[Message], query: &str, most: usize) -> Vec<Found> {
        let parts = self.parts(live);
        let words: BTreeSet<String> = search_words(query).into_iter().collect();
        let exact = query.trim().to_lowercase();
        let documents: Vec<Vec<String>> = parts.iter().map(|p| search_words(&p.text)).collect();
        let count = documents.len() as f64;
        let average = documents.iter().map(Vec::len).sum::<usize>() as f64 / count.max(1.0);
        let holding: BTreeMap<&String, f64> = words
            .iter()
            .map(|word| {
                let held = documents.iter().filter(|d| d.contains(word)).count();
                (word, held as f64)
            })
            .collect();
        let mut scored: Vec<(bool, f64, usize)> = Vec::new();
        for (at, document) in documents.iter().enumerate() {
            let mut score = 0.0;
            for word in &words {
                let frequency = document.iter().filter(|w| *w == word).count() as f64;
                if frequency == 0.0 {
                    continue;
                }
                let rarity = (1.0 + (count - holding[word] + 0.5) / (holding[word] + 0.5)).ln();
                let length = document.len() as f64 / average.max(1.0);
                score += rarity * frequency * 2.2 / (frequency + 1.2 * (0.25 + 0.75 * length));
            }
            let verbatim = exact.len() > 2 && parts[at].text.to_lowercase().contains(&exact);
            if score > 0.0 || verbatim {
                scored.push((verbatim, score, at));
            }
        }
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.total_cmp(&a.1)).then(a.2.cmp(&b.2)));
        scored
            .into_iter()
            .take(most)
            .map(|(_, _, at)| {
                let part = &parts[at];
                Found {
                    number: part.number,
                    what: part.what.clone(),
                    snippet: around(&part.text, &exact, &words, 240),
                    call: part.call.clone(),
                }
            })
            .collect()
    }

    /// The message numbered `number` as it was, from its character
    /// `offset`, at most [`READ_CHARS`] of it, and whether it holds a
    /// result from outside. An error if it is not kept.
    pub fn read(
        &self,
        live: &[Message],
        number: usize,
        offset: usize,
    ) -> Result<(String, bool), String> {
        let message = self.message(live, number)?;
        let tools = self.tools(live);
        let mut lines = Vec::new();
        let mut outside = false;
        for block in &message.content {
            match block {
                Block::Text { text } if message.role == Role::Assistant => {
                    lines.push(format!("You said: {text}"));
                }
                Block::Text { text } => lines.push(format!("The person said: {text}")),
                Block::ToolUse { name, input, .. } => {
                    lines.push(format!("You called {name} with {input}"));
                }
                Block::ToolResult {
                    id,
                    content,
                    is_error,
                    ..
                } => {
                    outside |= self.outside.contains(id);
                    lines.push(format!(
                        "The result of {}{}: {content}",
                        tools.get(id.as_str()).unwrap_or(&"a call"),
                        if *is_error { " (an error)" } else { "" }
                    ));
                }
                Block::Image { image } => lines.push(format!(
                    "A picture ({}): its bytes are not kept.",
                    image.media_type
                )),
                Block::Reasoning { .. } => {}
            }
        }
        let text = lines.join("\n");
        Ok((
            page(&format!("[Message {number}, as it was:]"), &text, offset),
            outside,
        ))
    }

    /// The result of the call `id` as it was, from its character `offset`,
    /// and whether it came from outside. An error if it is not kept.
    pub fn read_result(
        &self,
        live: &[Message],
        id: &str,
        offset: usize,
    ) -> Result<(String, bool), String> {
        // The one cut from a message still there, or else the latest let
        // go: where a model gave two calls one id, the one it last saw.
        let kept = self.cut.get(id).cloned().or_else(|| {
            self.messages
                .iter()
                .rev()
                .flat_map(|m| &m.content)
                .find_map(|block| match block {
                    Block::ToolResult {
                        id: of, content, ..
                    } if of == id => Some(content.clone()),
                    _ => None,
                })
        });
        let Some(content) = kept else {
            return Err(format!(
                "no result of a call {id:?} was let go: it is in the conversation as it is, or \
                 was never there"
            ));
        };
        let tools = self.tools(live);
        let head = format!(
            "[The result of {}, as it was:]",
            tools.get(id).unwrap_or(&"a call")
        );
        Ok((page(&head, &content, offset), self.outside.contains(id)))
    }

    /// The message numbered `number`, with the results cut from it given
    /// back.
    fn message(&self, live: &[Message], number: usize) -> Result<Message, String> {
        if number == 0 {
            return live
                .first()
                .cloned()
                .ok_or_else(|| "the conversation is empty".to_string());
        }
        if number <= self.dropped {
            return Err(format!(
                "message {number} is no longer kept: the {} oldest messages let go went for \
                 good when the session reached what it keeps",
                self.dropped
            ));
        }
        let at = number - self.dropped - 1;
        if let Some(message) = self.messages.get(at) {
            return Ok(message.clone());
        }
        let index = number - self.dropped - self.messages.len() + self.notes;
        live.get(index)
            .map(|message| self.restored(message))
            .ok_or_else(|| {
                format!(
                    "there is no message {number}: the conversation has {} so far",
                    self.dropped + self.messages.len() + live.len().saturating_sub(1 + self.notes)
                )
            })
    }

    /// Answers a call of [`LOOK_BACK`] with `input`, for a conversation
    /// that is now `live`, and records it.
    pub fn look(&mut self, live: &[Message], input: &Value) -> Looked {
        let offset = input["offset"]
            .as_u64()
            .and_then(|o| usize::try_from(o).ok())
            .unwrap_or(0);
        let query = input["query"].as_str().map(str::trim).unwrap_or_default();
        let result = input["result"].as_str().map(str::trim).unwrap_or_default();
        let (asked, answer, found) = if let Some(number) = input["message"]
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
        {
            let answer = self.read(live, number, offset);
            (format!("message {number}"), answer, vec![number])
        } else if !result.is_empty() {
            let answer = self.read_result(live, result, offset);
            (format!("the result of call {result}"), answer, Vec::new())
        } else if !query.is_empty() {
            let found = self.search(live, query, FOUND);
            let outside = found
                .iter()
                .any(|f| f.call.as_ref().is_some_and(|id| self.outside.contains(id)));
            let numbers = found.iter().map(|f| f.number).collect();
            let text = if found.is_empty() {
                format!(
                    "Nothing that was let go holds {query:?}. {}",
                    self.extent(live)
                )
            } else {
                let mut lines = vec![format!(
                    "[What was let go that best matches {query:?}, the best first:]"
                )];
                for f in &found {
                    lines.push(format!("- message {}, {}: {}", f.number, f.what, f.snippet));
                }
                lines.push("Read one whole with {\"message\": <its number>}.".to_string());
                lines.join("\n")
            };
            (query.to_string(), Ok((text, outside)), numbers)
        } else {
            (
                String::new(),
                Err(
                    "give `query` to search what was let go, `message` to read one by its \
                     number, or `result` to read a result that was cut by its call's id"
                        .to_string(),
                ),
                Vec::new(),
            )
        };
        self.looked += 1;
        if !asked.is_empty() {
            self.looks.push(Look {
                asked: clip(&asked, 200),
                read: input["message"].is_u64() || !result.is_empty(),
                found: if answer.is_ok() { found } else { Vec::new() },
            });
            let over = self.looks.len().saturating_sub(LOOKS_KEPT);
            self.looks.drain(..over);
        }
        match answer {
            Ok((content, outside)) => Looked {
                content,
                is_error: false,
                outside,
            },
            Err(why) => Looked {
                content: why,
                is_error: true,
                outside: false,
            },
        }
    }

    /// What is kept, in a sentence: for an answer that found nothing.
    fn extent(&self, live: &[Message]) -> String {
        let cut = live
            .iter()
            .flat_map(|m| &m.content)
            .filter(|b| cut_result(b).is_some_and(|id| self.cut.contains_key(id)))
            .count();
        match (self.messages.is_empty(), cut) {
            (true, 0) => "Nothing was let go: the conversation is all there.".to_string(),
            (true, cut) => format!("{cut} cut result(s) are kept."),
            (false, cut) => format!(
                "Messages {} to {} are kept{}.",
                self.dropped + 1,
                self.dropped + self.messages.len(),
                if cut > 0 {
                    format!(", and {cut} cut result(s)")
                } else {
                    String::new()
                }
            ),
        }
    }

    /// What shortening its conversation cost a session, in lines for
    /// whoever improves how it is kept: how often it was shortened and how
    /// many names were then out of view, what a model's notes made up, and what the
    /// agent looked back for. Empty if it was never shortened. Nothing a
    /// tool answered is in them: the agent's own words for what it looked
    /// for, and the names its notes' writer made up.
    pub fn cost(&self) -> Vec<String> {
        let mut lines = Vec::new();
        if let Some(last) = self.spans.last() {
            lines.push(format!(
                "its conversation was shortened {} time(s); after the last, {} name(s) of what \
                 it had let go were in neither its notes nor what it kept",
                self.spans.len(),
                last.not_held
            ));
        }
        let made_up: BTreeSet<&String> = self.spans.iter().flat_map(|s| &s.made_up).collect();
        if !made_up.is_empty() {
            lines.push(format!(
                "its notes named what is nowhere in what they replaced: {}",
                made_up
                    .into_iter()
                    .map(|name| clip(name, NAME_CHARS))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        for look in &self.looks {
            lines.push(if look.read {
                format!("it read {} again", look.asked)
            } else {
                format!(
                    "it searched what it had let go for {:?}{}",
                    look.asked,
                    if look.found.is_empty() {
                        " and found nothing"
                    } else {
                        ""
                    }
                )
            });
        }
        lines
    }

    /// The line that says what the conversation let go, for its notes:
    /// empty if nothing is kept to look back at.
    pub fn let_go_line(&self, look_back: bool) -> String {
        let Some(last) = self.last() else {
            return String::new();
        };
        let kept = if self.messages.is_empty() {
            "They are no longer kept.".to_string()
        } else if self.dropped > 0 {
            format!("Those from {} on are kept as they were.", self.dropped + 1)
        } else {
            "They are kept as they were.".to_string()
        };
        let how = if look_back && !self.messages.is_empty() {
            format!(
                " {LOOK_BACK} searches them by words and reads one by its number: look back \
                 rather than guess at a name, a number or what something said."
            )
        } else {
            String::new()
        };
        format!("{LET_GO} messages 1 to {last} of this conversation. {kept}{how}]")
    }
}

/// A model with the model that writes the notes of the conversations it
/// works in ([`Model::notes_writer`]): how a task run as a trial is given
/// its notes' writer.
pub struct WithNotes {
    /// The model.
    pub model: Box<dyn Model>,
    /// The one that writes its notes.
    pub notes: Box<dyn Model>,
}

impl std::fmt::Debug for WithNotes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "WithNotes({}, notes by {})",
            self.model.id(),
            self.notes.id()
        )
    }
}

impl Model for WithNotes {
    fn id(&self) -> &str {
        self.model.id()
    }

    fn complete(&self, request: &Request) -> Result<Response, ModelError> {
        self.model.complete(request)
    }

    fn stream(
        &self,
        request: &Request,
        seen: &mut dyn FnMut(Delta) -> bool,
    ) -> Result<Response, ModelError> {
        self.model.stream(request, seen)
    }

    fn position(&self) -> Option<usize> {
        self.model.position()
    }

    fn most_tokens(&self, max_tokens: u32) -> u32 {
        self.model.most_tokens(max_tokens)
    }

    fn endpoint(&self) -> Option<String> {
        self.model.endpoint()
    }

    fn notes_writer(&self) -> Option<&dyn Model> {
        Some(self.notes.as_ref())
    }
}

/// How the model sees [`LOOK_BACK`].
pub fn spec() -> ToolSpec {
    ToolSpec {
        name: LOOK_BACK.into(),
        description: "Look back at the part of this conversation that was let go to save \
                      context: it is kept as it was. Give `query` (words, a name, an identifier) \
                      to search it, `message` (a number) to read one message whole, or `result` \
                      (a call's id) to read a result that was cut. `offset` reads on from that \
                      character of a long one. Look back rather than guess at a name, a number \
                      or what something said."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "query": {"type": "string"},
                "message": {"type": "integer", "minimum": 0},
                "result": {"type": "string"},
                "offset": {"type": "integer", "minimum": 0}
            }
        }),
    }
}

/// The id of the call whose result `block` is, when that result was cut
/// from the conversation: what stands there is the runtime's line saying
/// so. A result that is still as it was is not one, whatever its id.
fn cut_result(block: &Block) -> Option<&str> {
    match block {
        Block::ToolResult { id, content, .. } if content.starts_with(crate::session::MASKED) => {
            Some(id.as_str())
        }
        _ => None,
    }
}

/// `text` from its character `offset`, at most [`READ_CHARS`] of it, under
/// `head`, with where to read on if there is more.
fn page(head: &str, text: &str, offset: usize) -> String {
    let count = text.chars().count();
    if offset >= count && count > 0 {
        return format!("{head}\n[Nothing from character {offset}: it has {count}.]");
    }
    let shown: String = text.chars().skip(offset).take(READ_CHARS).collect();
    let end = offset + shown.chars().count();
    if end < count {
        format!(
            "{head}\n{shown}\n[… {} more characters: look back again with \"offset\": {end}]",
            count - end
        )
    } else {
        format!("{head}\n{shown}")
    }
}

/// `text` on one line within `most` characters: its start and its end,
/// what is between them left out. How a result came out is often at its
/// end (a command's last lines, a page's conclusion).
fn clip_ends(text: &str, most: usize) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let count = line.chars().count();
    if count <= most {
        return line;
    }
    let tail = most * 2 / 5;
    let head: String = line.chars().take(most - tail).collect();
    let end: String = line.chars().skip(count - tail).collect();
    format!("{head} … {end}")
}

/// `text` on one line within `most` characters.
fn clip(text: &str, most: usize) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= most {
        line
    } else {
        let cut: String = line.chars().take(most).collect();
        format!("{cut}…")
    }
}

/// Where in `lower` (a text in lower case) a passage of about `span` bytes
/// holds the most of `words`: the first such place. None if it holds none.
fn densest(lower: &str, words: &BTreeSet<String>, span: usize) -> Option<usize> {
    let mut found: Vec<(usize, usize)> = Vec::new();
    for (which, word) in words.iter().enumerate() {
        found.extend(
            lower
                .match_indices(word.as_str())
                .map(|(at, _)| (at, which)),
        );
    }
    found.sort_unstable();
    let mut best: Option<(usize, usize)> = None;
    for (start, &(at, _)) in found.iter().enumerate() {
        let held: BTreeSet<usize> = found[start..]
            .iter()
            .take_while(|(later, _)| later - at <= span)
            .map(|(_, which)| *which)
            .collect();
        if best.map_or(true, |(most, _)| held.len() > most) {
            best = Some((held.len(), at));
        }
    }
    best.map(|(_, at)| at)
}

/// About `most` characters of `text`, on one line, around where it first
/// holds `exact` or, failing that, where it holds the most of `words`
/// close together.
fn around(text: &str, exact: &str, words: &BTreeSet<String>, most: usize) -> String {
    let lower = text.to_lowercase();
    let at = (!exact.is_empty())
        .then(|| lower.find(exact))
        .flatten()
        .or_else(|| densest(&lower, words, most * 2 / 3))
        .unwrap_or(0);
    // Lowercasing can change a text's length in bytes: count in characters.
    let before = lower[..at.min(lower.len())].chars().count();
    let start = before.saturating_sub(most / 3);
    let count = text.chars().count();
    let shown: String = text.chars().skip(start).take(most).collect();
    let line = shown.split_whitespace().collect::<Vec<_>>().join(" ");
    format!(
        "{}{line}{}",
        if start > 0 { "…" } else { "" },
        if start + most < count { "…" } else { "" }
    )
}

/// The names in `text`: what a reader could not write without having seen
/// it. Paths and addresses, file names, identifiers with a digit, an
/// underscore or `::` in them, numbers of five digits or more. Ordinary
/// words, short numbers and capitalised words are not names.
pub fn names(text: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for raw in text.split(|c: char| {
        c.is_whitespace()
            || matches!(
                c,
                '"' | '\''
                    | '`'
                    | '('
                    | ')'
                    | '['
                    | ']'
                    | '{'
                    | '}'
                    | '<'
                    | '>'
                    | ','
                    | ';'
                    | '='
                    | '|'
            )
    }) {
        let token = raw
            .trim_start_matches(['*', '#', '-', '+', ':', '!', '?', '\\'])
            .trim_end_matches(['*', '.', ':', '!', '?', '\\', '/', '-', '+']);
        let token = token.strip_prefix("./").unwrap_or(token);
        if is_name(token) {
            found.insert(token.to_string());
        }
    }
    found
}

/// Whether `token` is a name ([`names`]).
fn is_name(token: &str) -> bool {
    let count = token.chars().count();
    if !(4..=NAME_CHARS).contains(&count) {
        return false;
    }
    let letters = token.chars().filter(|c| c.is_alphabetic()).count();
    let digits = token.chars().filter(char::is_ascii_digit).count();
    if letters == 0 {
        // A number: a date, an amount, an account. Short ones are not
        // worth a name.
        return digits >= 5
            && token
                .chars()
                .all(|c| c.is_ascii_digit() || "-.:/".contains(c));
    }
    if token.contains("://") {
        return true;
    }
    if token.contains('@') {
        return token
            .rsplit('@')
            .next()
            .is_some_and(|host| host.contains('.'));
    }
    if token.contains('/') {
        // A path, not "and/or": it has more than one step, starts at a
        // root, or holds what a word does not.
        return token.matches('/').count() >= 2
            || token.starts_with(['/', '~'])
            || digits > 0
            || token.contains(['.', '_', '-']);
    }
    if token.contains("::") || token.contains('_') {
        return true;
    }
    if let Some((stem, ending)) = token.rsplit_once('.') {
        // A file's name or a host's: not a sentence's end run into the next.
        let plain = |part: &str| !part.is_empty() && part.chars().all(|c| c.is_alphanumeric());
        return plain(ending)
            && ending.chars().count() <= 8
            && stem.chars().any(char::is_alphanumeric)
            && (digits > 0
                || ending
                    .chars()
                    .all(|c| c.is_lowercase() || c.is_ascii_digit()));
    }
    if token.contains('-') {
        return digits > 0;
    }
    // An identifier: letters and digits together, more than a unit's worth
    // of either ("600ms" is not one).
    count >= 6 && letters >= 3 && digits >= 2
}

/// The names in `notes` that are nowhere in `source` (in lower case): what
/// their writer made up, or brought from somewhere it should not have.
pub fn unsupported(notes: &str, source: &str) -> Vec<String> {
    names(notes)
        .into_iter()
        .filter(|name| !source.contains(&name.to_lowercase()))
        .collect()
}

/// The text of `messages`, in lower case: what [`unsupported`] checks
/// against, and what [`not_held`] looks in.
pub fn lower_text(messages: &[Message]) -> String {
    let mut text = String::new();
    for message in messages {
        for block in &message.content {
            match block {
                Block::Text { text: t } => text.push_str(&t.to_lowercase()),
                Block::ToolUse { name, input, .. } => {
                    text.push_str(name);
                    text.push(' ');
                    text.push_str(&input.to_string().to_lowercase());
                }
                Block::ToolResult { content, .. } => text.push_str(&content.to_lowercase()),
                Block::Image { .. } | Block::Reasoning { .. } => {}
            }
            text.push('\n');
        }
    }
    text
}

/// The names of `let_go` that `kept` (in lower case) no longer holds: at
/// most `most` of them, those the person, the agent or its calls named
/// before those only results held, then by how many messages named them;
/// and how many there are in all.
pub fn not_held(let_go: &[Message], kept: &str, most: usize) -> (Vec<String>, usize) {
    let mut weights: BTreeMap<String, u64> = BTreeMap::new();
    for message in let_go {
        let mut said = BTreeSet::new();
        let mut returned = BTreeSet::new();
        for block in &message.content {
            match block {
                Block::Text { text } => said.extend(names(text)),
                Block::ToolUse { input, .. } => said.extend(names(&input.to_string())),
                Block::ToolResult { content, .. } => returned.extend(names(content)),
                Block::Image { .. } | Block::Reasoning { .. } => {}
            }
        }
        for name in &said {
            *weights.entry(name.clone()).or_default() += 1_000;
        }
        for name in returned.difference(&said) {
            *weights.entry(name.clone()).or_default() += 1;
        }
    }
    let mut missing: Vec<(u64, String)> = weights
        .into_iter()
        .filter(|(name, _)| !kept.contains(&name.to_lowercase()))
        .map(|(name, weight)| (weight, name))
        .collect();
    let all = missing.len();
    missing.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    (
        missing
            .into_iter()
            .take(most)
            .map(|(_, name)| name)
            .collect(),
        all,
    )
}

/// The line that lists the names notes do not hold: empty if there are
/// none to list.
pub fn not_held_line(listed: &[String], all: usize, look_back: bool) -> String {
    if listed.is_empty() {
        return String::new();
    }
    let more = all.saturating_sub(listed.len());
    format!(
        "{NOT_HELD} {}{}. They are in what was let go{}]",
        listed.join(", "),
        if more > 0 {
            format!(", and {more} more names")
        } else {
            String::new()
        },
        if look_back {
            format!(": {LOOK_BACK} finds them.")
        } else {
            ".".to_string()
        }
    )
}

/// The line that names what a model's notes made up.
pub fn made_up_line(names: &[String]) -> String {
    if names.is_empty() {
        return String::new();
    }
    format!(
        "{MADE_UP} {}. Do not rely on them.]",
        names
            .iter()
            .map(|name| clip(name, NAME_CHARS))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// What a writer whose notes named what is nowhere in what they replace is
/// asked, the second time.
pub fn ask_again(names: &[String]) -> String {
    format!(
        "These appear in your notes and nowhere in what you were given: {}. Write the notes \
         again. Keep everything that is supported; leave out, or mark as not known, whatever \
         is not.",
        names.join(", ")
    )
}

/// A name the agent went on to use that its shortened conversation no
/// longer held ([`Replayed`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lost {
    /// The name.
    pub name: String,
    /// The number of the message that used it.
    pub used_at: usize,
}

/// The most lost names a replay keeps: the first ones.
pub const LOST_KEPT: usize = 60;

/// What playing a recorded conversation again showed of a way of keeping
/// one short ([`crate::session::replay`]).
///
/// At each point where it would have been shortened, the names the agent
/// went on to use that it could know only from what came before are
/// counted, and of those the ones the shortened conversation still held.
/// A name is what [`names`] finds: a path, an address, an identifier, a
/// long number. A name that a later result or the person gave again before
/// the agent used it is not counted: it did not need to be kept.
///
/// This counts names, not what was understood, and only what one recorded
/// run went on to use. It says how much of that a way of keeping the
/// conversation would have left in view; it does not say how the agent
/// would have done.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Replayed {
    /// The conversation's messages.
    pub messages: usize,
    /// How many times it was shortened.
    pub shortenings: usize,
    /// The names the agent went on to use from what came before, summed
    /// over the shortenings.
    pub needed: usize,
    /// Of those, the ones the shortened conversation still held: in its
    /// notes, in what the runtime keeps beside them, in the task or in the
    /// messages kept as they are.
    pub held: usize,
    /// Of the others, the ones the notes listed as let go
    /// ([`not_held_line`]): in view by name, to be looked back for.
    #[serde(default)]
    pub listed: usize,
    /// The ones not held, each once with the message that used it: the
    /// first [`LOST_KEPT`].
    pub lost: Vec<Lost>,
    /// The conversation's characters before each shortening, summed.
    pub before: usize,
    /// And after.
    pub after: usize,
    /// The tokens the notes' writer used, if a model wrote them.
    pub tokens: u64,
}

impl Replayed {
    /// Adds what another conversation's replay showed.
    pub fn add(&mut self, other: &Replayed) {
        self.messages += other.messages;
        self.shortenings += other.shortenings;
        self.needed += other.needed;
        self.held += other.held;
        self.listed += other.listed;
        self.before += other.before;
        self.after += other.after;
        self.tokens += other.tokens;
        for lost in &other.lost {
            if self.lost.len() < LOST_KEPT && !self.lost.contains(lost) {
                self.lost.push(lost.clone());
            }
        }
    }
}

/// `text` without the line that lists the names notes do not hold
/// ([`not_held_line`]), and that line: a name listed there is in view, but
/// nothing of what it is.
pub fn without_listed(text: &str) -> (String, String) {
    // In whatever case: what is searched for names is in lower case.
    let marker = NOT_HELD.to_lowercase();
    let (listed, rest): (Vec<&str>, Vec<&str>) = text
        .lines()
        .partition(|line| line.to_lowercase().starts_with(&marker));
    (rest.join("\n"), listed.join("\n"))
}

/// The names `message` holds that the agent wrote: in what it said and in
/// the inputs of its calls. None for a message that is not the agent's.
pub fn names_used(message: &Message) -> BTreeSet<String> {
    let mut used = BTreeSet::new();
    if message.role != Role::Assistant {
        return used;
    }
    for block in &message.content {
        match block {
            Block::Text { text } => used.extend(names(text)),
            Block::ToolUse { input, .. } => used.extend(names(&input.to_string())),
            _ => {}
        }
    }
    used
}

/// How a long conversation is kept short: what an improvement loop changes
/// of it, and measures.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Keeping {
    /// What the notes' writer is told it is for.
    pub told: String,
    /// What it is asked, after the messages its notes replace.
    pub asked: String,
    /// Messages kept as they are at the end.
    pub keep_last: usize,
    /// The longest result of a call long past that is kept whole when the
    /// conversation is first cut down, in bytes.
    pub cut_over: usize,
    /// How many of the names the notes do not hold are listed after them.
    pub names: usize,
}

/// The most characters of what a notes' writer is told, or asked.
pub const MAX_KEEPING_CHARS: usize = 2_000;

impl Default for Keeping {
    fn default() -> Self {
        Keeping {
            told: crate::session::SUMMARY_SYSTEM.to_string(),
            asked: crate::session::SUMMARY_ASK.to_string(),
            keep_last: 12,
            cut_over: crate::session::MASK_OVER,
            names: NAMES_LISTED,
        }
    }
}

impl Keeping {
    /// How `limits` keep a conversation.
    pub fn of(limits: &crate::session::Limits) -> Keeping {
        Keeping {
            told: limits
                .notes_told
                .clone()
                .unwrap_or_else(|| crate::session::SUMMARY_SYSTEM.to_string()),
            asked: limits
                .notes_asked
                .clone()
                .unwrap_or_else(|| crate::session::SUMMARY_ASK.to_string()),
            keep_last: limits.keep_last,
            cut_over: limits.cut_over,
            names: limits.names_listed,
        }
    }

    /// Makes `limits` keep a conversation this way.
    pub fn apply(&self, limits: &mut crate::session::Limits) {
        let default = Keeping::default();
        limits.notes_told = (self.told != default.told).then(|| self.told.clone());
        limits.notes_asked = (self.asked != default.asked).then(|| self.asked.clone());
        limits.keep_last = self.keep_last;
        limits.cut_over = self.cut_over;
        limits.names_listed = self.names;
    }

    /// Whether it is within what a session takes: why not, if it is not.
    pub fn check(&self) -> Result<(), String> {
        for (what, text) in [
            ("what the notes' writer is told", &self.told),
            ("what it is asked", &self.asked),
        ] {
            if text.trim().is_empty() {
                return Err(format!("{what} is empty"));
            }
            if text.chars().count() > MAX_KEEPING_CHARS {
                return Err(format!(
                    "{what} is longer than {MAX_KEEPING_CHARS} characters"
                ));
            }
        }
        if !(2..=64).contains(&self.keep_last) {
            return Err("it keeps between 2 and 64 messages as they are".into());
        }
        if !(100..=20_000).contains(&self.cut_over) {
            return Err(
                "the results it cuts first are those longer than 100 to 20,000 characters".into(),
            );
        }
        if self.names > 64 {
            return Err("it lists at most 64 of the names its notes do not hold".into());
        }
        Ok(())
    }

    /// What it changes of `from`, in a line.
    pub fn change_from(&self, from: &Keeping) -> String {
        let mut parts = Vec::new();
        if self.told.trim() != from.told.trim() {
            parts.push(format!(
                "tells its notes' writer {:?}",
                clip(&self.told, 80)
            ));
        }
        if self.asked.trim() != from.asked.trim() {
            parts.push(format!("asks it {:?}", clip(&self.asked, 80)));
        }
        if self.keep_last != from.keep_last {
            parts.push(format!(
                "keeps its last {} messages as they are (it kept {})",
                self.keep_last, from.keep_last
            ));
        }
        if self.cut_over != from.cut_over {
            parts.push(format!(
                "cuts old results longer than {} characters first (it cut those over {})",
                self.cut_over, from.cut_over
            ));
        }
        if self.names != from.names {
            parts.push(format!(
                "lists {} of the names its notes do not hold (it listed {})",
                self.names, from.names
            ));
        }
        if parts.is_empty() {
            "keeps its conversation as before".into()
        } else {
            parts.join(", ")
        }
    }
}

/// The mechanical notes: `previous` (the notes before these, if they were
/// mechanical too), then what was said and called in `messages` and what
/// came back, a shortened line each: of a result, its start and its end. Past [`MECHANICAL_CHARS`] the oldest
/// lines go, and are counted. `stand_for` is how many messages the notes
/// stand for in all.
pub fn mechanical(previous: &str, messages: &[Message], stand_for: usize) -> String {
    let mut lines: Vec<String> = previous
        .lines()
        .filter(|line| !line.starts_with("[Summary of ") && !line.starts_with("[… "))
        .map(str::to_string)
        .collect();
    let earlier_gone: usize = previous
        .lines()
        .find_map(|line| {
            line.strip_prefix("[… ")?
                .split_whitespace()
                .next()?
                .parse()
                .ok()
        })
        .unwrap_or(0);
    for message in messages {
        for block in &message.content {
            lines.push(match block {
                Block::Text { text } if message.role == Role::Assistant => {
                    format!("- I said: {}", clip(text, 240))
                }
                Block::Text { text } => format!("- I was told: {}", clip(text, 240)),
                Block::ToolUse { name, input, .. } => {
                    format!("- I called {name} with {}", clip(&input.to_string(), 160))
                }
                Block::ToolResult {
                    content, is_error, ..
                } => format!(
                    "  → {}{}",
                    if *is_error { "error: " } else { "" },
                    clip_ends(content, 200)
                ),
                Block::Image { image } => format!("- I was shown a picture ({})", image.media_type),
                // Kept reasoning is the model's own, and opaque: not summed up.
                Block::Reasoning { .. } => continue,
            });
        }
    }
    let mut chars: usize = lines.iter().map(|line| line.len() + 1).sum();
    let mut gone = 0;
    while chars > MECHANICAL_CHARS && gone + 1 < lines.len() {
        chars -= lines[gone].len() + 1;
        gone += 1;
    }
    let mut notes = vec![format!(
        "[Summary of {stand_for} earlier messages, compacted to save context]"
    )];
    if gone + earlier_gone > 0 {
        notes.push(format!(
            "[… {} earlier lines of this summary let go …]",
            gone + earlier_gone
        ));
    }
    notes.extend(lines.into_iter().skip(gone));
    notes.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(role: Role, text: &str) -> Message {
        Message {
            role,
            content: vec![Block::text(text)],
        }
    }

    fn called(id: &str, tool: &str, input: Value) -> Message {
        Message {
            role: Role::Assistant,
            content: vec![Block::ToolUse {
                id: id.into(),
                name: tool.into(),
                input,
                signature: None,
            }],
        }
    }

    fn gave(id: &str, content: &str) -> Message {
        Message {
            role: Role::User,
            content: vec![Block::ToolResult {
                id: id.into(),
                content: content.into(),
                is_error: false,
                images: Vec::new(),
            }],
        }
    }

    #[test]
    fn names_are_what_a_reader_could_not_write_unseen() {
        let found = names(
            "Read notes/a.txt and https://example.com/x, then INV-7731 (ref a8a5977ba) in \
             crate::earlier, the config_path, v1.2.3, 2026-11-16 and bob@example.com. Paid \
             600ms later, and/or 12 times: well-known. See `src/main.rs`; {\"path\":\"b.md\"}.",
        );
        for name in [
            "notes/a.txt",
            "https://example.com/x",
            "INV-7731",
            "a8a5977ba",
            "crate::earlier",
            "config_path",
            "v1.2.3",
            "2026-11-16",
            "bob@example.com",
            "src/main.rs",
            "b.md",
        ] {
            assert!(found.contains(name), "{name} of {found:?}");
        }
        for word in ["600ms", "and/or", "well-known", "Read", "times", "12"] {
            assert!(!found.contains(word), "{word} of {found:?}");
        }
    }

    #[test]
    fn a_name_nowhere_in_what_notes_replace_is_unsupported() {
        let source = lower_text(&[
            said(Role::User, "Pay invoice INV-7731 from notes/a.txt."),
            called("c1", "read_file", json!({"path": "notes/a.txt"})),
        ]);
        assert!(unsupported("Read notes/a.txt; INV-7731 is to be paid.", &source).is_empty());
        // A path written shorter, or with ./ before it, is the same path.
        assert!(unsupported("Read ./notes/a.txt and a.txt.", &source).is_empty());
        assert_eq!(
            unsupported("Paid INV-7732 as notes/b.txt says.", &source),
            vec!["INV-7732".to_string(), "notes/b.txt".to_string()]
        );
    }

    #[test]
    fn what_is_let_go_is_kept_whole_and_numbered() {
        let live = vec![
            said(Role::User, "the task"),
            called("c1", "read_file", json!({"path": "a.txt"})),
            gave("c1", "[Cut to save context: the result of read_file]"),
            said(Role::Assistant, "It says alpha."),
            said(Role::User, "And b?"),
        ];
        let mut earlier = Earlier::default();
        earlier.keep_cut("c1", "alpha beta gamma");
        // Before anything is let go, numbers are places.
        assert_eq!(earlier.number(2), Some(2));
        let (text, _) = earlier.read(&live, 2, 0).unwrap();
        assert!(
            text.contains("The result of read_file: alpha beta gamma"),
            "{text}"
        );
        // The first three after the task are let go: the cut result is whole
        // again in what is kept, and no longer kept twice.
        earlier.let_go(&live[1..4], EARLIER_CHARS);
        earlier.notes = 1;
        let now = vec![
            live[0].clone(),
            said(Role::Assistant, "[notes]"),
            live[4].clone(),
        ];
        assert!(earlier.cut.is_empty());
        assert_eq!(earlier.last(), Some(3));
        assert_eq!(earlier.number(1), None);
        assert_eq!(earlier.number(2), Some(4));
        let whole = earlier.whole(&now);
        assert_eq!(whole.len(), 5);
        assert_eq!(whole[2], gave("c1", "alpha beta gamma"));
        assert_eq!(whole[4], live[4]);
        let (text, outside) = earlier.read(&now, 4, 0).unwrap();
        assert!(
            text.contains("The person said: And b?") && !outside,
            "{text}"
        );
        assert!(earlier.read(&now, 9, 0).is_err());
    }

    #[test]
    fn a_search_finds_what_was_let_go_and_a_read_gives_it_back() {
        let mut earlier = Earlier::default();
        earlier.from_outside("c2");
        earlier.let_go(
            &[
                called("c1", "read_file", json!({"path": "invoice.txt"})),
                gave(
                    "c1",
                    "Invoice INV-7731 from Acme: 8 USD, to be paid to carol.",
                ),
                called("c2", "fetch", json!({"url": "https://example.com/terms"})),
                gave("c2", "Terms: payment within thirty days."),
                said(Role::Assistant, "The invoice is from Acme."),
            ],
            EARLIER_CHARS,
        );
        let live = vec![said(Role::User, "the task")];
        let found = earlier.search(&live, "who is to be paid", FOUND);
        assert_eq!(found[0].number, 2, "{found:?}");
        assert!(found[0].snippet.contains("paid to carol"), "{found:?}");
        // An identifier is found as it is written.
        let found = earlier.search(&live, "INV-7731", FOUND);
        assert_eq!(found[0].number, 2);
        assert!(earlier.search(&live, "zebra", FOUND).is_empty());

        let looked = earlier.look(&live, &json!({"query": "payment terms"}));
        assert!(!looked.is_error && looked.outside, "{looked:?}");
        assert!(looked.content.contains("message 4, the result of fetch"));
        let looked = earlier.look(&live, &json!({"message": 2}));
        assert!(looked.content.contains("to be paid to carol") && !looked.outside);
        let looked = earlier.look(&live, &json!({"result": "c2"}));
        assert!(looked.content.contains("thirty days") && looked.outside);
        assert!(earlier.look(&live, &json!({})).is_error);
        assert_eq!(earlier.looked, 4);
        assert_eq!(earlier.looks.len(), 3);
        assert_eq!(earlier.looks[0].found, vec![4, 3]);
    }

    #[test]
    fn a_match_is_shown_where_the_most_of_its_words_are_and_a_result_by_both_ends() {
        let mut earlier = Earlier::default();
        let sheet = format!(
            "Order sheet. {} The door code is blue heron. {} Order ORD-4401 ships today.",
            "Packed and weighed at the depot. ".repeat(12),
            "Nothing more to note. ".repeat(12)
        );
        earlier.let_go(&[gave("c1", &sheet)], EARLIER_CHARS);
        let live = vec![said(Role::User, "the task")];
        // "order" is in its first words; what was asked about is further in.
        let found = earlier.search(&live, "door code of the order", FOUND);
        assert!(
            found[0].snippet.contains("door code is blue heron"),
            "{found:?}"
        );
        // Mechanical notes keep how a result began and how it ended.
        let notes = mechanical("", &[gave("c1", &sheet)], 1);
        assert!(
            notes.contains("  → Order sheet. Packed")
                && notes.ends_with("Order ORD-4401 ships today."),
            "{notes}"
        );
        assert!(!notes.contains("blue heron") && notes.contains(" … "));
        assert!(notes.len() < 300, "{}", notes.len());
    }

    #[test]
    fn a_long_message_is_read_a_page_at_a_time() {
        let mut earlier = Earlier::default();
        let long = "x".repeat(READ_CHARS + 500);
        earlier.let_go(&[gave("c1", &long)], EARLIER_CHARS);
        let live = vec![said(Role::User, "the task")];
        let (first, _) = earlier.read_result(&live, "c1", 0).unwrap();
        assert!(
            first.contains(&format!(
                "500 more characters: look back again with \"offset\": {READ_CHARS}"
            )),
            "{}",
            &first[first.len() - 120..]
        );
        let (rest, _) = earlier.read_result(&live, "c1", READ_CHARS).unwrap();
        assert!(rest.ends_with(&"x".repeat(500)) && !rest.contains("more characters"));
    }

    #[test]
    fn past_what_a_session_keeps_the_oldest_go_and_are_counted() {
        let mut earlier = Earlier::default();
        earlier.from_outside("c1");
        earlier.let_go(
            &[gave("c1", &"a".repeat(600)), gave("c2", &"b".repeat(600))],
            1_000,
        );
        assert_eq!((earlier.dropped, earlier.messages.len()), (1, 1));
        assert!(!earlier.is_whole() && earlier.outside.is_empty());
        let live = vec![said(Role::User, "the task")];
        assert!(earlier
            .read(&live, 1, 0)
            .unwrap_err()
            .contains("no longer kept"));
        assert!(earlier.read(&live, 2, 0).is_ok());
        assert!(earlier
            .let_go_line(true)
            .contains("Those from 2 on are kept"));
    }

    #[test]
    fn the_names_notes_do_not_hold_are_listed_those_said_first() {
        let let_go = vec![
            said(Role::User, "Pay INV-7731."),
            called("c1", "read_file", json!({"path": "ledger/2026.csv"})),
            gave("c1", "rows: acct_00417, acct_00418, acct_00419"),
        ];
        let (listed, all) = not_held(&let_go, "notes: paid inv-7731.", 2);
        assert_eq!(all, 4);
        assert_eq!(listed, vec!["ledger/2026.csv", "acct_00417"]);
        let line = not_held_line(&listed, all, true);
        assert!(
            line.starts_with("[Not in these notes: ledger/2026.csv, acct_00417, and 2 more names."),
            "{line}"
        );
        assert!(not_held_line(&[], 0, true).is_empty());
    }

    #[test]
    fn mechanical_notes_carry_the_ones_before_and_let_their_oldest_lines_go() {
        let first = mechanical(
            "",
            &[said(Role::User, "one"), said(Role::Assistant, "two")],
            2,
        );
        assert_eq!(
            first,
            "[Summary of 2 earlier messages, compacted to save context]\n- I was told: one\n- I said: two"
        );
        let second = mechanical(&first, &[said(Role::Assistant, "three")], 3);
        assert!(
            second.starts_with("[Summary of 3 earlier messages"),
            "{second}"
        );
        assert!(second.ends_with("- I was told: one\n- I said: two\n- I said: three"));
        let many: Vec<Message> = (0..200)
            .map(|n| said(Role::Assistant, &format!("line {n} {}", "y".repeat(100))))
            .collect();
        let long = mechanical(&second, &many, 203);
        assert!(long.len() <= MECHANICAL_CHARS + 200, "{}", long.len());
        assert!(
            long.contains("earlier lines of this summary let go"),
            "{long}"
        );
        assert!(long.ends_with(&format!("line 199 {}", "y".repeat(100))));
        // What went is counted across summaries.
        let gone = |notes: &str| -> usize {
            notes
                .lines()
                .find_map(|l| {
                    l.strip_prefix("[… ")?
                        .split_whitespace()
                        .next()?
                        .parse()
                        .ok()
                })
                .unwrap()
        };
        let again = mechanical(&long, &many, 403);
        assert!(gone(&again) > gone(&long));
    }

    #[test]
    fn how_a_conversation_is_kept_is_checked_and_described() {
        let default = Keeping::default();
        default.check().unwrap();
        let mut limits = crate::session::Limits::default();
        assert_eq!(Keeping::of(&limits), default);
        let changed = Keeping {
            asked: "Keep every amount and who is to be paid.".into(),
            keep_last: 6,
            ..default.clone()
        };
        changed.apply(&mut limits);
        assert_eq!(limits.keep_last, 6);
        assert_eq!(limits.notes_told, None);
        assert_eq!(Keeping::of(&limits), changed);
        assert_eq!(
            changed.change_from(&default),
            "asks it \"Keep every amount and who is to be paid.\", keeps its last 6 messages as \
             they are (it kept 12)"
        );
        assert!(Keeping {
            keep_last: 1,
            ..default.clone()
        }
        .check()
        .is_err());
        assert!(Keeping {
            asked: " ".into(),
            ..default.clone()
        }
        .check()
        .is_err());
        assert!(Keeping {
            told: "x".repeat(2_001),
            ..default
        }
        .check()
        .is_err());
    }
}
