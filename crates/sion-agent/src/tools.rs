//! Tools: what a call may do, and the toolbox a session draws from.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::model::ToolSpec;

/// What a tool's call can do to the world, from least to most.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    /// Reads only: files, the ledger, memory.
    Read,
    /// Changes the agent's own workspace: its files, its memory.
    Write,
    /// Reaches outside: the network, messages to people or other agents.
    External,
    /// Moves money (within the agent's charter, which the ledger enforces).
    Spend,
}

/// How a tool keeps a call's effect from happening twice when the call is
/// made again after its outcome was lost: what makes the journal's
/// "nothing is done twice" hold for that tool, or not.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dedup {
    /// Making the call again changes nothing more (writing the same content
    /// to the same place): it may be made again.
    Idempotent,
    /// The service it calls drops a repeat of the call's key
    /// ([`Tool::call`]'s `key`): it may be made again.
    Keyed,
    /// What the call did can be found from what it noted while it ran (a
    /// transaction's digest and the last Pulse it may execute in): an
    /// unknown outcome is reconciled ([`Tool::reconcile`]) before the call
    /// is made again or answered.
    Reconciled,
    /// Nothing stops the effect happening twice: a call whose outcome is
    /// unknown is not made again until a person settles it.
    AtLeastOnce,
}

/// What a call returned.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outcome {
    /// What the model sees.
    pub content: String,
    /// Whether the call failed.
    pub is_error: bool,
    /// Whether the outcome is known. False when the call may or may not have
    /// taken effect (a timeout after a payment was sent): such a call is
    /// never repeated automatically.
    pub certain: bool,
    /// Whether the content came from outside the agent's trust (a web page,
    /// another agent's message). It taints the session.
    pub untrusted: bool,
    /// Whether the content is confidential (a file the operator named, a
    /// report from a session that held such content): the session holds it
    /// from then on, and its external calls and spending wait for a person.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub confidential: bool,
    /// The calls a skill pre-approves now that it was read: the skill's
    /// name and its `allowed-tools` rules (see [`crate::permissions`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pre_approval: Option<(String, Vec<crate::permissions::ToolRule>)>,
    /// Pictures the model is shown with the content (a screenshot). They
    /// are not journaled: the session keeps the last few.
    #[serde(skip)]
    pub pictures: Vec<Picture>,
    /// Tokens the call's own models used (subagents at work): counted in
    /// the run's budget.
    #[serde(skip)]
    pub spent: Option<crate::model::Usage>,
}

/// A picture a tool returned: as a person sends one.
pub use crate::model::Picture;

/// What a session holds that its calls carry on to what they start (the
/// subagents a tool spawns): content from outside, and confidential
/// content.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Held {
    /// The session read content from outside (it is tainted).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub tainted: bool,
    /// The session holds confidential content.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub confidential: bool,
}

impl Outcome {
    /// A success.
    pub fn ok(content: impl Into<String>) -> Outcome {
        Outcome {
            content: content.into(),
            is_error: false,
            certain: true,
            untrusted: false,
            confidential: false,
            pre_approval: None,
            pictures: Vec::new(),
            spent: None,
        }
    }

    /// A failure that changed nothing.
    pub fn error(content: impl Into<String>) -> Outcome {
        Outcome {
            content: content.into(),
            is_error: true,
            certain: true,
            untrusted: false,
            confidential: false,
            pre_approval: None,
            pictures: Vec::new(),
            spent: None,
        }
    }

    /// A call whose effect is unknown.
    pub fn uncertain(content: impl Into<String>) -> Outcome {
        Outcome {
            content: content.into(),
            is_error: true,
            certain: false,
            untrusted: false,
            confidential: false,
            pre_approval: None,
            pictures: Vec::new(),
            spent: None,
        }
    }

    /// Adds a picture (`media_type`, its bytes in base64) the model is
    /// shown with the content.
    pub fn with_picture(mut self, media_type: &str, base64: String) -> Outcome {
        self.pictures.push(Picture {
            media_type: media_type.to_string(),
            base64,
        });
        self
    }

    /// Marks the content as untrusted.
    pub fn from_outside(mut self) -> Outcome {
        self.untrusted = true;
        self
    }

    /// Marks the content as confidential ([`Outcome::confidential`]).
    pub fn held_confidential(mut self) -> Outcome {
        self.confidential = true;
        self
    }

    /// Says that reading this pre-approves the calls `rules` match, for the
    /// skill `skill`, for the rest of the session.
    pub fn pre_approving(
        mut self,
        skill: &str,
        rules: Vec<crate::permissions::ToolRule>,
    ) -> Outcome {
        if !rules.is_empty() {
            self.pre_approval = Some((skill.to_string(), rules));
        }
        self
    }
}

/// A tool an agent can call.
pub trait Tool: Send + Sync {
    /// How the model sees it.
    fn spec(&self) -> ToolSpec;

    /// The most a call can do.
    fn effect(&self) -> Effect;

    /// What the call with `input` can do: at most [`Tool::effect`], less
    /// when the input shows it (a skill's script that has no network).
    fn effect_of(&self, _input: &Value) -> Effect {
        self.effect()
    }

    /// Whether the journal answers a repeat of an identical call with the
    /// first one's outcome instead of making it again. True for calls that
    /// add to the world (a payment, a message); a tool whose identical
    /// calls are meant to run again (a shell command after a fix, writing
    /// a file back as it was) says false, and its calls are not journaled.
    fn journaled(&self) -> bool {
        true
    }

    /// Runs one call. `key` identifies the call (see [`crate::journal::key`]):
    /// a tool that talks to a service which deduplicates requests passes it
    /// on.
    fn call(&self, input: &Value, key: &str) -> Outcome;

    /// Runs one call of a session that read content from outside (it is
    /// tainted): by default as [`Tool::call`]. A tool that records what it
    /// is given, such as memory, marks it as coming from such a session.
    fn call_tainted(&self, input: &Value, key: &str) -> Outcome {
        self.call(input, key)
    }

    /// Whether the calls of this tool that follow one another in a turn run
    /// together ([`Tool::call_batch`]), each once it passed the policy by
    /// itself: subagents that work at the same time, say.
    fn batches(&self) -> bool {
        false
    }

    /// Runs several calls together: one outcome each, in order. By
    /// default, one after another.
    fn call_batch(&self, calls: &[(&Value, &str)]) -> Vec<Outcome> {
        calls
            .iter()
            .map(|(input, key)| self.call(input, key))
            .collect()
    }

    /// [`Tool::call_batch`] for a session that read content from outside:
    /// by default, each call as [`Tool::call_tainted`].
    fn call_batch_tainted(&self, calls: &[(&Value, &str)]) -> Vec<Outcome> {
        calls
            .iter()
            .map(|(input, key)| self.call_tainted(input, key))
            .collect()
    }

    /// [`Tool::call_batch`] (or [`Tool::call_batch_tainted`], for a
    /// session that `held` tainted) within a run that has `budget` tokens
    /// left, if it has a budget: a tool whose calls run models of their own
    /// (subagents) keeps them within it, says what they used
    /// ([`Outcome::spent`]), and starts them holding what the session holds.
    /// By default the budget is not the tool's concern.
    fn call_batch_budgeted(
        &self,
        calls: &[(&Value, &str)],
        held: Held,
        budget: Option<u64>,
    ) -> Vec<Outcome> {
        let _ = budget;
        if held.tainted {
            self.call_batch_tainted(calls)
        } else {
            self.call_batch(calls)
        }
    }

    /// Whether what it returns from outside may be held in quarantine
    /// ([`crate::quarantine`]): by default, yes. A tool whose results the
    /// model acts on (a browser's page, whose elements it clicks) says no,
    /// and its results taint the session as before.
    fn quarantinable(&self) -> bool {
        true
    }

    /// What the call with `input` reads or changes, named the same by every
    /// call of it (`file:notes/a.txt`), if the tool can say: the session
    /// keeps the latest outcome for each apart from the conversation
    /// ([`crate::session::State::latest`]), so that compacting the
    /// conversation does not lose what is now so. By default, nothing.
    fn resource(&self, input: &Value) -> Option<String> {
        let _ = input;
        None
    }

    /// How the tool keeps a call's effect from happening twice
    /// ([`Dedup`]): by default, nothing does.
    fn dedup(&self) -> Dedup {
        Dedup::AtLeastOnce
    }

    /// Runs one call as [`Tool::call`] (or [`Tool::call_tainted`]), telling
    /// `note` what it learns on the way that would later show what it did
    /// (a transaction's digest, before the transaction is sent): the
    /// runtime keeps each note in the journal, on disk, before the call
    /// ends. By default, nothing is noted.
    fn call_noting(
        &self,
        input: &Value,
        key: &str,
        tainted: bool,
        note: &mut dyn FnMut(&str),
    ) -> Outcome {
        let _ = note;
        if tainted {
            self.call_tainted(input, key)
        } else {
            self.call(input, key)
        }
    }

    /// What a call whose outcome is unknown did, found from the `notes` it
    /// left ([`Tool::call_noting`]): a certain outcome (an error if it did
    /// nothing, so that it may be made again), or none if that cannot be
    /// told yet. Asked only of a [`Dedup::Reconciled`] tool.
    fn reconcile(&self, input: &Value, key: &str, notes: &[String]) -> Option<Outcome> {
        let _ = (input, key, notes);
        None
    }
}

/// The tools of a session, by name.
#[derive(Default)]
pub struct Toolbox {
    tools: BTreeMap<String, Box<dyn Tool>>,
    /// The tools a large toolbox shows only once a search finds them
    /// ([`Toolbox::add_deferred`]).
    deferred: BTreeSet<String>,
}

impl std::fmt::Debug for Toolbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.tools.keys()).finish()
    }
}

/// Above this many tools, the model is not shown them all: it sees the
/// tools that are always shown (all but the deferred ones, if they are no
/// more than this), two meta-tools, one to search the toolbox and one to
/// call a tool it found, and the tools its searches found and loaded
/// ([`Toolbox::shown_with`]).
pub const META_THRESHOLD: usize = 40;

/// The most tools a session's searches keep loaded, the latest found.
pub const MAX_LOADED: usize = 24;

/// The most tools one search returns, and loads.
pub const MAX_FOUND: usize = 10;

/// The tools one search returns unless asked for fewer or more.
pub const FOUND_BY_DEFAULT: usize = 5;

/// The meta-tool that searches the toolbox.
pub const TOOLS_SEARCH: &str = "tools_search";
/// The meta-tool that calls a tool by name.
pub const TOOLS_CALL: &str = "tools_call";

impl Toolbox {
    /// An empty toolbox.
    pub fn new() -> Toolbox {
        Toolbox::default()
    }

    /// Adds a tool, replacing any of the same name.
    pub fn add(&mut self, tool: impl Tool + 'static) {
        self.add_boxed(Box::new(tool));
    }

    /// Adds a boxed tool.
    pub fn add_boxed(&mut self, tool: Box<dyn Tool>) {
        let name = tool.spec().name;
        self.deferred.remove(&name);
        self.tools.insert(name, tool);
    }

    /// Adds a tool that a large toolbox shows only once a search finds it
    /// (an MCP server's, say), so that the tools always shown stay in view.
    pub fn add_deferred(&mut self, tool: Box<dyn Tool>) {
        let name = tool.spec().name;
        self.deferred.insert(name.clone());
        self.tools.insert(name, tool);
    }

    /// Whether a large toolbox shows the tool named only once a search
    /// finds it.
    pub fn is_deferred(&self, name: &str) -> bool {
        self.deferred.contains(name)
    }

    /// A tool by name.
    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(|t| t.as_ref())
    }

    /// The tools' names, sorted.
    pub fn names(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }

    /// Keeps only the tools whose names `keep` keeps.
    pub fn retain(&mut self, keep: impl Fn(&str) -> bool) {
        self.tools.retain(|name, _| keep(name));
        let tools = &self.tools;
        self.deferred.retain(|name| tools.contains_key(name));
    }

    /// Every tool's spec, sorted by name.
    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools.values().map(|t| t.spec()).collect()
    }

    /// What the model is shown: every tool, or, past [`META_THRESHOLD`],
    /// the two meta-tools ([`Toolbox::shown_with`]).
    pub fn shown(&self) -> Vec<ToolSpec> {
        self.shown_with(&[])
    }

    /// What the model is shown, with the tools its searches loaded
    /// (`loaded`, by name): every tool, or, past [`META_THRESHOLD`], the
    /// tools always shown (all but the deferred ones, if they are no more
    /// than the threshold; otherwise none), the two meta-tools, then the
    /// loaded tools, the last [`MAX_LOADED`] of them, in the order they
    /// were found: so that loading more leaves the start of the list as it
    /// was, for providers' caches.
    pub fn shown_with(&self, loaded: &[String]) -> Vec<ToolSpec> {
        if self.tools.len() <= META_THRESHOLD {
            return self.specs();
        }
        let always: Vec<&Box<dyn Tool>> = self
            .tools
            .iter()
            .filter(|(name, _)| !self.deferred.contains(*name))
            .map(|(_, tool)| tool)
            .collect();
        let mut shown: Vec<ToolSpec> = if always.len() <= META_THRESHOLD {
            always.iter().map(|tool| tool.spec()).collect()
        } else {
            Vec::new()
        };
        shown.extend(self.meta_tools());
        let skip = loaded.len().saturating_sub(MAX_LOADED);
        for name in &loaded[skip..] {
            if shown.iter().any(|spec| &spec.name == name) {
                continue;
            }
            if let Some(tool) = self.tools.get(name) {
                shown.push(tool.spec());
            }
        }
        shown
    }

    /// The two meta-tools of a large toolbox.
    fn meta_tools(&self) -> Vec<ToolSpec> {
        vec![
            ToolSpec {
                name: TOOLS_SEARCH.into(),
                description: format!(
                    "Search the {} tools available by what they do, in words (or by name: \
                     select:<name>,<name>). The best matches are returned with their input \
                     schemas and loaded: from then on they may be called directly by name.",
                    self.tools.len()
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "query": {"type": "string"},
                        "limit": {"type": "integer", "minimum": 1, "maximum": MAX_FOUND}
                    },
                    "required": ["query"]
                }),
            },
            ToolSpec {
                name: TOOLS_CALL.into(),
                description: "Call a tool found with tools_search: its name, and its input as \
                              its schema describes."
                    .into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {"name": {"type": "string"}, "input": {"type": "object"}},
                    "required": ["name", "input"]
                }),
            },
        ]
    }

    /// The tools that best match `query`, at most [`FOUND_BY_DEFAULT`]
    /// ([`Toolbox::ranked`]).
    pub fn search(&self, query: &str) -> Vec<ToolSpec> {
        self.ranked(query, FOUND_BY_DEFAULT)
    }

    /// The `limit` tools (at most [`MAX_FOUND`]) that best match `query`,
    /// the best first: a tool named in the query comes before the rest
    /// (`select:<name>,<name>` names tools and asks for nothing else), and
    /// the rest are ranked by BM25 over the words of their names (counted
    /// three times), descriptions and inputs' names and descriptions, with
    /// camelCase and snake_case split, a plural's `s` taken off and common
    /// words left out. A tool that shares no word with the query is not
    /// returned.
    pub fn ranked(&self, query: &str, limit: usize) -> Vec<ToolSpec> {
        let limit = limit.clamp(1, MAX_FOUND);
        let query = query.trim();
        if let Some(names) = query.strip_prefix("select:") {
            return names
                .split(',')
                .filter_map(|name| self.tools.get(name.trim()))
                .map(|tool| tool.spec())
                .take(limit)
                .collect();
        }
        let specs = self.specs();
        let named: Vec<usize> = query
            .split(|c: char| c.is_whitespace() || c == ',')
            .filter_map(|word| specs.iter().position(|spec| spec.name == word))
            .collect();
        let words = search_words(query);
        let documents: Vec<Vec<String>> = specs.iter().map(searched_words).collect();
        let count = documents.len() as f64;
        let average = documents.iter().map(Vec::len).sum::<usize>() as f64 / count.max(1.0);
        let mut scored: Vec<(f64, usize)> = Vec::new();
        for (at, document) in documents.iter().enumerate() {
            let mut score = 0.0;
            for word in words.iter().collect::<BTreeSet<_>>() {
                let frequency = document.iter().filter(|w| *w == word).count() as f64;
                if frequency == 0.0 {
                    continue;
                }
                let holding = documents.iter().filter(|d| d.contains(word)).count() as f64;
                let rarity = (1.0 + (count - holding + 0.5) / (holding + 0.5)).ln();
                let length = document.len() as f64 / average.max(1.0);
                score += rarity * frequency * (BM25_K1 + 1.0)
                    / (frequency + BM25_K1 * (1.0 - BM25_B + BM25_B * length));
            }
            if score > 0.0 && !named.contains(&at) {
                scored.push((score, at));
            }
        }
        // The best first; of two as good, the one first by name.
        scored.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        let mut seen = BTreeSet::new();
        named
            .into_iter()
            .chain(scored.into_iter().map(|(_, at)| at))
            .filter(|at| seen.insert(*at))
            .take(limit)
            .map(|at| specs[at].clone())
            .collect()
    }

    /// How many tools there are.
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

/// BM25's saturation of a word's count, for [`Toolbox::ranked`].
const BM25_K1: f64 = 1.2;

/// BM25's weight of a document's length, for [`Toolbox::ranked`].
const BM25_B: f64 = 0.75;

/// Words too common to search by.
const COMMON_WORDS: [&str; 30] = [
    "a", "an", "and", "are", "as", "at", "be", "by", "can", "for", "from", "i", "in", "into", "is",
    "it", "its", "me", "my", "of", "on", "or", "that", "the", "this", "to", "use", "with", "you",
    "your",
];

/// The words of `text` a search goes by: lowercase, split where it is not
/// a letter or digit and where camelCase turns, a plural's `s` taken off,
/// common words and single letters left out.
pub fn search_words(text: &str) -> Vec<String> {
    fn end(word: &mut String, words: &mut Vec<String>) {
        if word.is_empty() {
            return;
        }
        let mut done = std::mem::take(word);
        if done.len() > 3 && done.ends_with('s') && !done.ends_with("ss") {
            done.pop();
        }
        // A letter alone (the s of an apostrophe's) says nothing.
        if done.chars().count() > 1 && !COMMON_WORDS.contains(&done.as_str()) {
            words.push(done);
        }
    }
    let mut words = Vec::new();
    let mut word = String::new();
    let mut lower = false;
    for c in text.chars() {
        if c.is_alphanumeric() {
            if c.is_uppercase() && lower {
                end(&mut word, &mut words);
            }
            lower = c.is_lowercase() || c.is_numeric();
            word.extend(c.to_lowercase());
        } else {
            end(&mut word, &mut words);
            lower = false;
        }
    }
    end(&mut word, &mut words);
    words
}

/// What a search of the toolbox reads of a tool: the words of its name,
/// three times, of its description, and of its inputs' names and
/// descriptions.
fn searched_words(spec: &ToolSpec) -> Vec<String> {
    let name = search_words(&spec.name);
    let mut words: Vec<String> = name.iter().chain(&name).chain(&name).cloned().collect();
    words.extend(search_words(&spec.description));
    for (input, about) in spec.input_schema["properties"]
        .as_object()
        .into_iter()
        .flatten()
    {
        words.extend(search_words(input));
        words.extend(search_words(
            about["description"].as_str().unwrap_or_default(),
        ));
    }
    words
}

/// A tool made of a spec, an effect and a function: for tools that need no
/// state of their own.
pub struct FnTool<F> {
    spec: ToolSpec,
    effect: Effect,
    f: F,
}

impl<F> std::fmt::Debug for FnTool<F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FnTool({})", self.spec.name)
    }
}

impl<F: Fn(&Value) -> Outcome + Send + Sync> FnTool<F> {
    /// A tool named `name`.
    pub fn new(name: &str, description: &str, schema: Value, effect: Effect, f: F) -> Self {
        FnTool {
            spec: ToolSpec {
                name: name.into(),
                description: description.into(),
                input_schema: schema,
            },
            effect,
            f,
        }
    }
}

impl<F: Fn(&Value) -> Outcome + Send + Sync> Tool for FnTool<F> {
    fn spec(&self) -> ToolSpec {
        self.spec.clone()
    }

    fn effect(&self) -> Effect {
        self.effect
    }

    fn call(&self, input: &Value, _key: &str) -> Outcome {
        (self.f)(input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str, description: &str) -> FnTool<impl Fn(&Value) -> Outcome + Send + Sync> {
        FnTool::new(
            name,
            description,
            json!({"type": "object"}),
            Effect::Read,
            |_| Outcome::ok(""),
        )
    }

    #[test]
    fn a_large_toolbox_is_shown_through_two_meta_tools() {
        let mut tools = Toolbox::new();
        for i in 0..META_THRESHOLD {
            tools.add(tool(&format!("tool_{i}"), "does a thing"));
        }
        assert_eq!(tools.shown().len(), META_THRESHOLD);
        tools.add(tool("weather", "today's forecast for a city"));
        let shown: Vec<String> = tools.shown().into_iter().map(|s| s.name).collect();
        assert_eq!(shown, vec![TOOLS_SEARCH, TOOLS_CALL]);
        let found = tools.search("Forecast city");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "weather");
    }

    #[test]
    fn a_search_ranks_tools_by_what_they_do() {
        let mut tools = Toolbox::new();
        tools.add(tool("gmail_send", "Sends an email message to recipients"));
        tools.add(tool("gmail_search", "Searches the mailbox for messages"));
        tools.add(tool(
            "calendarCreateEvent",
            "Creates an event in a calendar",
        ));
        tools.add(tool("weather", "Today's forecast for a city"));
        let names = |found: Vec<ToolSpec>| -> Vec<String> {
            found.into_iter().map(|spec| spec.name).collect()
        };
        // Every word need not match; the best come first.
        assert_eq!(
            names(tools.ranked("send a message to Ana", 5)),
            ["gmail_send", "gmail_search"]
        );
        // camelCase and plurals are split and made one.
        assert_eq!(
            names(tools.ranked("create calendar events", 5)),
            ["calendarCreateEvent"]
        );
        // A tool named comes first; select: asks for those alone.
        assert_eq!(names(tools.ranked("weather messages", 5))[0], "weather");
        assert_eq!(
            names(tools.ranked("select:weather,gmail_send,nothing", 5)),
            ["weather", "gmail_send"]
        );
        assert!(tools.ranked("quantum chromodynamics", 5).is_empty());
        assert_eq!(tools.ranked("messages", 1).len(), 1, "as many as asked for");
        assert_eq!(
            search_words("listOpenPullRequests, the user's"),
            ["list", "open", "pull", "request", "user"]
        );
    }

    #[test]
    fn a_large_toolbox_keeps_its_own_tools_in_view_and_shows_what_a_search_loaded() {
        let mut tools = Toolbox::new();
        for name in ["read_file", "write_file", "shell"] {
            tools.add(tool(name, "a tool of its own"));
        }
        for i in 0..META_THRESHOLD {
            tools.add_deferred(Box::new(tool(&format!("mcp_{i:02}"), "a server's tool")));
        }
        assert!(tools.is_deferred("mcp_07") && !tools.is_deferred("shell"));
        let names = |shown: Vec<ToolSpec>| -> Vec<String> {
            shown.into_iter().map(|spec| spec.name).collect()
        };
        assert_eq!(
            names(tools.shown()),
            ["read_file", "shell", "write_file", TOOLS_SEARCH, TOOLS_CALL]
        );
        // What searches loaded comes after, in the order found, the
        // latest kept; a name that is no tool, or one shown already, is not
        // shown twice.
        let loaded: Vec<String> = ["mcp_03", "shell", "gone", "mcp_01"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            names(tools.shown_with(&loaded)),
            [
                "read_file",
                "shell",
                "write_file",
                TOOLS_SEARCH,
                TOOLS_CALL,
                "mcp_03",
                "mcp_01"
            ]
        );
        let many: Vec<String> = (0..MAX_LOADED + 3).map(|i| format!("mcp_{i:02}")).collect();
        let shown = names(tools.shown_with(&many));
        assert_eq!(shown.len(), 5 + MAX_LOADED);
        assert!(!shown.contains(&"mcp_00".to_string()) && shown.contains(&"mcp_26".to_string()));
        // A tool added again as one of its own is shown as one.
        tools.add(tool("mcp_05", "now its own"));
        assert!(!tools.is_deferred("mcp_05"));
        assert!(names(tools.shown()).contains(&"mcp_05".to_string()));
    }

    #[test]
    fn a_toolbox_names_its_tools_and_keeps_only_those_it_is_told_to() {
        let mut tools = Toolbox::new();
        for name in ["pay", "balance", "fetch"] {
            tools.add(tool(name, "does a thing"));
        }
        assert_eq!(tools.names(), ["balance", "fetch", "pay"]);
        tools.retain(|name| name != "pay");
        assert_eq!(tools.names(), ["balance", "fetch"]);
        assert!(tools.get("pay").is_none());
    }
}
