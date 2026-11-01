//! Tools: what a call may do, and the toolbox a session draws from.

use std::collections::BTreeMap;

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

impl Outcome {
    /// A success.
    pub fn ok(content: impl Into<String>) -> Outcome {
        Outcome {
            content: content.into(),
            is_error: false,
            certain: true,
            untrusted: false,
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

    /// [`Tool::call_batch`] (or [`Tool::call_batch_tainted`]) within a run
    /// that has `budget` tokens left, if it has a budget: a tool whose calls
    /// run models of their own (subagents) keeps them within it, and says
    /// what they used ([`Outcome::spent`]). By default the budget is not
    /// the tool's concern.
    fn call_batch_budgeted(
        &self,
        calls: &[(&Value, &str)],
        tainted: bool,
        budget: Option<u64>,
    ) -> Vec<Outcome> {
        let _ = budget;
        if tainted {
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
}

impl std::fmt::Debug for Toolbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.tools.keys()).finish()
    }
}

/// Above this many tools, the model sees two meta-tools instead of all of
/// them: one to search the toolbox and one to call a tool it found.
pub const META_THRESHOLD: usize = 40;

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
        self.tools.insert(tool.spec().name, Box::new(tool));
    }

    /// Adds a boxed tool.
    pub fn add_boxed(&mut self, tool: Box<dyn Tool>) {
        self.tools.insert(tool.spec().name, tool);
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
    }

    /// Every tool's spec, sorted by name.
    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools.values().map(|t| t.spec()).collect()
    }

    /// What the model is shown: every tool, or, past [`META_THRESHOLD`],
    /// the two meta-tools.
    pub fn shown(&self) -> Vec<ToolSpec> {
        if self.tools.len() <= META_THRESHOLD {
            return self.specs();
        }
        vec![
            ToolSpec {
                name: TOOLS_SEARCH.into(),
                description: format!(
                    "Search the {} tools available by words in their names and descriptions; \
                     returns matching tools with their input schemas.",
                    self.tools.len()
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": {"query": {"type": "string"}},
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

    /// The tools whose name or description contains every word of `query`.
    pub fn search(&self, query: &str) -> Vec<ToolSpec> {
        let words: Vec<String> = query.split_whitespace().map(|w| w.to_lowercase()).collect();
        self.specs()
            .into_iter()
            .filter(|s| {
                let hay = format!("{} {}", s.name, s.description).to_lowercase();
                words.iter().all(|w| hay.contains(w))
            })
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
