//! Searching what an agent did before: a full-text index over the messages
//! of its past sessions, ranked by BM25.
//!
//! Each message of each session is one document: what was said, the calls
//! the model made (a tool's name and its input) and what the tools
//! answered, each answer cut to [`MAX_RESULT_CHARS`]. Text becomes terms:
//! lowercase runs of letters and digits, without a short list of English
//! stop words, with a few English endings taken off (`-ing`, `-ed`, `-es`,
//! `-s`), so that "invoices" finds "invoice". Scores are BM25 (k₁ = 1.2,
//! b = 0.75) over the query's distinct terms. [`Index::search`] gives each
//! session once, at its best message, with the number of its messages that
//! match and a snippet around the first match; ties go to the later
//! session.
//!
//! The index lives in memory and is built from the sessions it is given;
//! where the sessions are kept, and whether a session still matches what
//! its owner recorded about it, is for the caller (the `sion` command
//! checks each hit against the trajectory digest the agent's memory log
//! committed to the ledger).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::model::{Block, Message, Role};

/// The most characters of a tool's answer that are indexed.
pub const MAX_RESULT_CHARS: usize = 2_000;

/// The longest snippet, in characters (before the ellipses).
pub const SNIPPET_CHARS: usize = 240;

/// The longest query, in terms.
pub const MAX_QUERY_TERMS: usize = 32;

const K1: f64 = 1.2;
const B: f64 = 0.75;

/// Words too common to search for.
const STOP: [&str; 48] = [
    "a", "an", "and", "are", "as", "at", "be", "but", "by", "did", "do", "does", "for", "from",
    "had", "has", "have", "he", "her", "his", "i", "if", "in", "into", "is", "it", "its", "me",
    "my", "no", "not", "of", "on", "or", "our", "she", "so", "that", "the", "their", "then",
    "they", "this", "to", "was", "we", "with", "you",
];

/// The terms of `text`, in order, repeats included.
pub fn terms(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for word in text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
    {
        let word = word.to_lowercase();
        if STOP.contains(&word.as_str()) {
            continue;
        }
        out.push(stem(&word));
    }
    out
}

/// `word` without a common English ending, if at least three characters
/// are left: plurals (`-s`, `-es` after a sibilant, `-ies` to `-y`) and
/// `-ing` and `-ed`. Words in `-ss`, `-us` and `-is` keep their `s`.
fn stem(word: &str) -> String {
    let len = word.chars().count();
    let cut = |n: usize| word[..word.len() - n].to_string();
    if !word.is_ascii() || len < 4 {
        return word.to_string();
    }
    if word.ends_with("sses") {
        return cut(2);
    }
    if word.ends_with("ies") && len >= 5 {
        return cut(3) + "y";
    }
    for sibilant in ["xes", "ches", "shes", "zes"] {
        if word.ends_with(sibilant) && len >= sibilant.len() + 2 {
            return cut(2);
        }
    }
    if word.ends_with("ing") && len >= 6 {
        return cut(3);
    }
    if word.ends_with("ed") && len >= 5 {
        return cut(2);
    }
    if word.ends_with('s') && !["ss", "us", "is"].iter().any(|e| word.ends_with(e)) {
        return cut(1);
    }
    word.to_string()
}

/// Who said a message, as a hit reports it.
fn role_name(message: &Message) -> &'static str {
    let answers = message
        .content
        .iter()
        .all(|b| matches!(b, Block::ToolResult { .. }));
    match message.role {
        Role::User if answers && !message.content.is_empty() => "tools",
        Role::User => "person",
        Role::Assistant => "agent",
    }
}

/// What of a message is indexed: its text, its calls and its tools'
/// answers.
pub fn message_text(message: &Message) -> String {
    let mut parts = Vec::new();
    for block in &message.content {
        match block {
            Block::Text { text } => parts.push(text.clone()),
            Block::ToolUse { name, input, .. } => parts.push(format!("{name} {input}")),
            Block::ToolResult { content, .. } => {
                parts.push(content.chars().take(MAX_RESULT_CHARS).collect())
            }
        }
    }
    parts.join("\n")
}

/// One message, indexed.
#[derive(Clone, Debug)]
struct Doc {
    session: usize,
    message: usize,
    role: &'static str,
    text: String,
    counts: BTreeMap<String, u32>,
    len: u32,
}

/// A session given to the index.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInfo {
    /// The session's identifier.
    pub id: String,
    /// When it last changed, for ordering ties (any monotone clock).
    pub at: u64,
    /// The task it started with.
    pub task: String,
}

/// A session that matched.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Hit {
    /// The session.
    pub session: SessionInfo,
    /// Its best message's position in the conversation.
    pub message: usize,
    /// Who said that message: `person`, `agent` or `tools`.
    pub role: String,
    /// Its score (BM25; for ordering and display only).
    pub score: f64,
    /// How many of the session's messages match.
    pub matches: usize,
    /// The text around the first match.
    pub snippet: String,
}

/// A full-text index over sessions' messages.
#[derive(Clone, Debug, Default)]
pub struct Index {
    sessions: Vec<SessionInfo>,
    docs: Vec<Doc>,
    df: BTreeMap<String, u32>,
    total_len: u64,
}

impl Index {
    /// An empty index.
    pub fn new() -> Index {
        Index::default()
    }

    /// Adds a session's messages.
    pub fn add(&mut self, session: SessionInfo, messages: &[Message]) {
        let index = self.sessions.len();
        self.sessions.push(session);
        for (position, message) in messages.iter().enumerate() {
            let text = message_text(message);
            let mut counts: BTreeMap<String, u32> = BTreeMap::new();
            let mut len = 0u32;
            for term in terms(&text) {
                *counts.entry(term).or_default() += 1;
                len = len.saturating_add(1);
            }
            if len == 0 {
                continue;
            }
            for term in counts.keys() {
                *self.df.entry(term.clone()).or_default() += 1;
            }
            self.total_len += u64::from(len);
            self.docs.push(Doc {
                session: index,
                message: position,
                role: role_name(message),
                text,
                counts,
                len,
            });
        }
    }

    /// How many sessions it holds.
    pub fn sessions(&self) -> usize {
        self.sessions.len()
    }

    /// How many messages it indexed (messages without terms are left out).
    pub fn messages(&self) -> usize {
        self.docs.len()
    }

    /// The sessions that best match `query`, at most `limit`, best first.
    pub fn search(&self, query: &str, limit: usize) -> Vec<Hit> {
        let wanted: BTreeSet<String> = terms(query).into_iter().take(MAX_QUERY_TERMS).collect();
        if wanted.is_empty() || self.docs.is_empty() || limit == 0 {
            return Vec::new();
        }
        let n = self.docs.len() as f64;
        let average = self.total_len as f64 / n;
        let idf: BTreeMap<&String, f64> = wanted
            .iter()
            .filter_map(|t| {
                let df = f64::from(*self.df.get(t)?);
                Some((t, ((n - df + 0.5) / (df + 0.5) + 1.0).ln()))
            })
            .collect();
        if idf.is_empty() {
            return Vec::new();
        }
        // The best message of each session, and how many matched.
        let mut best: BTreeMap<usize, (f64, usize, usize)> = BTreeMap::new();
        for (d, doc) in self.docs.iter().enumerate() {
            let mut score = 0.0;
            for (term, weight) in &idf {
                if let Some(&tf) = doc.counts.get(*term) {
                    let tf = f64::from(tf);
                    let norm = K1 * (1.0 - B + B * f64::from(doc.len) / average);
                    score += weight * tf * (K1 + 1.0) / (tf + norm);
                }
            }
            if score <= 0.0 {
                continue;
            }
            let entry = best.entry(doc.session).or_insert((score, d, 0));
            entry.2 += 1;
            if score > entry.0 {
                entry.0 = score;
                entry.1 = d;
            }
        }
        let mut ranked: Vec<(usize, f64, usize, usize)> = best
            .into_iter()
            .map(|(session, (score, doc, matches))| (session, score, doc, matches))
            .collect();
        ranked.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| self.sessions[b.0].at.cmp(&self.sessions[a.0].at))
                .then_with(|| self.sessions[a.0].id.cmp(&self.sessions[b.0].id))
        });
        ranked
            .into_iter()
            .take(limit)
            .map(|(session, score, doc, matches)| {
                let doc = &self.docs[doc];
                Hit {
                    session: self.sessions[session].clone(),
                    message: doc.message,
                    role: doc.role.to_string(),
                    score,
                    matches,
                    snippet: snippet(&doc.text, &wanted),
                }
            })
            .collect()
    }
}

/// The text around the first word of `text` whose term is wanted, on one
/// line, cut at word boundaries.
pub fn snippet(text: &str, wanted: &BTreeSet<String>) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars: Vec<char> = flat.chars().collect();
    // The first wanted word's start, in characters.
    let mut first = None;
    let mut start = None;
    for (i, c) in chars.iter().enumerate() {
        if c.is_alphanumeric() {
            start.get_or_insert(i);
        } else if let Some(s) = start.take() {
            let word: String = chars[s..i].iter().collect();
            if wanted.contains(&stem(&word.to_lowercase())) {
                first = Some(s);
                break;
            }
        }
    }
    if first.is_none() {
        if let Some(s) = start {
            let word: String = chars[s..].iter().collect();
            if wanted.contains(&stem(&word.to_lowercase())) {
                first = Some(s);
            }
        }
    }
    let at = first.unwrap_or(0);
    let mut from = at.saturating_sub(SNIPPET_CHARS / 3);
    while from > 0 && !chars[from - 1].is_whitespace() {
        from -= 1;
    }
    let mut to = (from + SNIPPET_CHARS).min(chars.len());
    while to < chars.len() && !chars[to].is_whitespace() {
        to += 1;
    }
    let mut out = String::new();
    if from > 0 {
        out.push('…');
    }
    out.extend(&chars[from..to]);
    if to < chars.len() {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn info(id: &str, at: u64) -> SessionInfo {
        SessionInfo {
            id: id.into(),
            at,
            task: format!("task of {id}"),
        }
    }

    fn assistant(text: &str) -> Message {
        Message {
            role: Role::Assistant,
            content: vec![Block::text(text)],
        }
    }

    #[test]
    fn terms_drop_stop_words_and_common_endings() {
        assert_eq!(
            terms("Paying the invoices of ACME-Ltd, 40 USD"),
            ["pay", "invoice", "acme", "ltd", "40", "usd"]
        );
        for (a, b) in [
            ("invoice", "invoices"),
            ("check", "checked"),
            ("check", "checking"),
            ("policy", "policies"),
            ("match", "matches"),
            ("box", "boxes"),
            ("class", "classes"),
        ] {
            assert_eq!(stem(a), stem(b), "{a} and {b}");
        }
        // Short words keep their endings, and so do -ss, -us and -is.
        assert_eq!(
            terms("is bus class sing status analysis used"),
            ["bus", "class", "sing", "status", "analysis", "used"]
        );
    }

    #[test]
    fn a_search_ranks_sessions_by_their_best_message() {
        let mut index = Index::new();
        index.add(
            info("s1", 1),
            &[
                Message::user("Pay invoice 7 from Acme, 40 USD."),
                assistant("I checked the payee with lookup_payee: acme-ltd."),
            ],
        );
        index.add(
            info("s2", 2),
            &[
                Message::user("Summarise the weather report."),
                assistant("Sunny, then rain."),
            ],
        );
        index.add(
            info("s3", 3),
            &[Message::user(
                "Which invoices did we pay to acme-ltd last week?",
            )],
        );
        let hits = index.search("acme invoice", 10);
        let ids: Vec<&str> = hits.iter().map(|h| h.session.id.as_str()).collect();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"s1") && ids.contains(&"s3"));
        assert!(hits.iter().all(|h| h.score > 0.0));
        let s1 = hits.iter().find(|h| h.session.id == "s1").unwrap();
        assert_eq!(s1.matches, 2);
        assert!(index.search("weather", 10)[0].session.id == "s2");
        assert!(index.search("the of and", 10).is_empty());
        assert!(index.search("nothing-like-this", 10).is_empty());
        assert_eq!(index.search("acme", 1).len(), 1);
    }

    #[test]
    fn calls_and_answers_are_searched_and_roles_named() {
        let mut index = Index::new();
        index.add(
            info("s1", 1),
            &[
                Message::user("Check the balance."),
                Message {
                    role: Role::Assistant,
                    content: vec![Block::ToolUse {
                        id: "c1".into(),
                        name: "ledger_balance".into(),
                        input: json!({"account": "treasury"}),
                    }],
                },
                Message {
                    role: Role::User,
                    content: vec![Block::ToolResult {
                        id: "c1".into(),
                        content: "treasury holds 1,234.5 USD".into(),
                        is_error: false,
                        images: Vec::new(),
                    }],
                },
            ],
        );
        let hit = &index.search("treasury holds", 5)[0];
        assert_eq!((hit.message, hit.role.as_str()), (2, "tools"));
        assert!(hit.snippet.contains("treasury holds 1,234.5 USD"));
        assert_eq!(index.search("ledger_balance", 5)[0].role, "agent");
        assert_eq!(index.search("check balance", 5)[0].role, "person");
    }

    #[test]
    fn ties_go_to_the_later_session() {
        let mut index = Index::new();
        index.add(info("early", 1), &[Message::user("rotate the keys")]);
        index.add(info("late", 9), &[Message::user("rotate the keys")]);
        let hits = index.search("rotate keys", 5);
        assert_eq!(hits[0].session.id, "late");
        assert_eq!(hits[1].session.id, "early");
    }

    #[test]
    fn snippets_are_cut_at_words_around_the_first_match() {
        let long = format!(
            "{} the payee is acme-ltd {}",
            "word ".repeat(100),
            "tail ".repeat(100)
        );
        let wanted: BTreeSet<String> = terms("payee").into_iter().collect();
        let s = snippet(&long, &wanted);
        assert!(s.starts_with('…') && s.ends_with('…'));
        assert!(s.contains("payee is acme-ltd"));
        assert!(s.chars().count() <= SNIPPET_CHARS + 2 + 8);
        assert!(!s.contains("wor…") && !s.contains("ta…"));
        // A match at the very end, and none at all.
        assert_eq!(
            snippet("pay acme", &terms("acme").into_iter().collect()),
            "pay acme"
        );
        assert_eq!(snippet("short text", &wanted), "short text");
    }

    #[test]
    fn a_tools_long_answer_is_indexed_only_in_part() {
        let mut index = Index::new();
        let answer = format!("{} needle", "hay ".repeat(MAX_RESULT_CHARS));
        index.add(
            info("s", 1),
            &[Message {
                role: Role::User,
                content: vec![Block::ToolResult {
                    id: "c".into(),
                    content: answer,
                    is_error: false,
                    images: Vec::new(),
                }],
            }],
        );
        assert!(index.search("needle", 5).is_empty());
        assert_eq!(index.search("hay", 5).len(), 1);
    }
}
