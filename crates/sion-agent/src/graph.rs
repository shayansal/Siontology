//! Graphs of agents: nodes that each run a session of their own, and edges
//! that say which run next, with what.
//!
//! A single agent with good tools is the default, and the right one for
//! most tasks: on the same budget, a graph of agents does better only when
//! the task divides (parts that need no one else's work, a check that
//! someone other than the writer makes). So a graph is something an
//! operator chooses, or a router proposes for a task that divides, and it
//! is small: a few nodes, each an agent session with its own instructions
//! and tools, and edges that pass a node's answer on.
//!
//! An edge is followed always, when a check passes or fails, when a field
//! of a node's JSON answer has a value, or once for each item of a list in
//! it (fanning out). A [`Kind::Join`] node waits until nothing that leads
//! to it is still to run, then runs once with every answer that reached
//! it. A node may run again (a loop back after a failed check) up to its
//! `max_visits`, and the whole graph runs at most `max_runs` nodes: a graph
//! that would go further stops, saying where.
//!
//! The run is kept as a [`GraphRun`] that serialises: it advances a wave at
//! a time ([`GraphRun::step`]), the nodes of a wave running together, and a
//! run picked up after a crash goes on from the last wave kept.
//!
//! An answer from a session that read content from outside may be steered
//! by that content: the nodes it reaches run as content from outside too
//! ([`Activation::outside`]), as a tainted parent's subagents do.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// A graph of agents.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Graph {
    /// Its name.
    pub name: String,
    /// The node that begins.
    pub start: String,
    /// The node whose answer is the graph's, if not the last to finish.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<String>,
    /// Its nodes.
    pub nodes: Vec<Node>,
    /// Its edges.
    #[serde(default)]
    pub edges: Vec<Edge>,
    /// The most node runs in all, loops included.
    #[serde(default = "default_max_runs")]
    pub max_runs: u32,
}

fn default_max_runs() -> u32 {
    24
}

fn default_max_visits() -> u32 {
    3
}

/// What a node is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// An agent's session, on each activation.
    #[default]
    Agent,
    /// An agent's session that waits for everything that leads to it, and
    /// runs once with every answer that reached it.
    Join,
}

/// A node: an agent, with its instructions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Node {
    /// Its id, unique in the graph.
    pub id: String,
    /// What it is.
    #[serde(default)]
    pub kind: Kind,
    /// What it is asked to do, before the task and its inputs.
    pub instructions: String,
    /// The only tools it may use, if not every tool of the profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<BTreeSet<String>>,
    /// The model it runs with, if not the profile's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// A JSON schema its answer keeps to, if its answer steers the graph
    /// (a check's `{"pass": …}`, a list to fan out over).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<Value>,
    /// The most times it runs.
    #[serde(default = "default_max_visits")]
    pub max_visits: u32,
}

/// When an edge is followed.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum When {
    /// Always.
    #[default]
    Always,
    /// When the node's answer says `"pass": true`.
    Pass,
    /// When it says `"pass": false`.
    Fail,
    /// When its answer's `field` is `equals`.
    Field {
        /// The field.
        field: String,
        /// The value.
        equals: Value,
    },
    /// Once for each item of its answer's list `field`, each run told its
    /// item.
    Each {
        /// The list.
        field: String,
    },
}

/// An edge: after `from`, `to`, when [`When`] holds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Edge {
    /// The node it leaves.
    pub from: String,
    /// The node it reaches.
    pub to: String,
    /// When it is followed.
    #[serde(default)]
    pub when: When,
}

/// The most nodes a graph has.
pub const MAX_NODES: usize = 16;
/// The most runs a fan-out starts.
pub const MAX_FAN_OUT: usize = 12;

impl Graph {
    /// The node `id`.
    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }

    /// Checks that the graph can run: ids unique and short, edges between
    /// its nodes, the start and the end among them, every node reachable
    /// from the start, a condition on a node's answer only where it keeps
    /// to a schema, and limits within reason.
    pub fn check(&self) -> Result<(), String> {
        if self.nodes.is_empty() || self.nodes.len() > MAX_NODES {
            return Err(format!("a graph has 1 to {MAX_NODES} nodes"));
        }
        let mut ids = BTreeSet::new();
        for node in &self.nodes {
            let valid = !node.id.is_empty()
                && node.id.len() <= 32
                && node
                    .id
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
            if !valid {
                return Err(format!(
                    "{:?} is not a node id: lowercase letters, digits and hyphens",
                    node.id
                ));
            }
            if !ids.insert(node.id.as_str()) {
                return Err(format!("{} is named twice", node.id));
            }
            if node.instructions.trim().is_empty() {
                return Err(format!("{} has no instructions", node.id));
            }
            if node.max_visits == 0 || node.max_visits as usize > MAX_FAN_OUT {
                return Err(format!("{}: max_visits is 1 to {MAX_FAN_OUT}", node.id));
            }
        }
        if self.max_runs == 0 || self.max_runs > 100 {
            return Err("max_runs is 1 to 100".into());
        }
        for id in std::iter::once(&self.start).chain(self.end.iter()) {
            if !ids.contains(id.as_str()) {
                return Err(format!("{id} is not a node of the graph"));
            }
        }
        for edge in &self.edges {
            for id in [&edge.from, &edge.to] {
                if !ids.contains(id.as_str()) {
                    return Err(format!("an edge names {id}, which is not a node"));
                }
            }
            let steered = !matches!(edge.when, When::Always);
            let from = self.node(&edge.from).expect("checked");
            if steered && from.output.is_none() {
                return Err(format!(
                    "{} → {} depends on {}'s answer, which keeps to no schema (give it `output`)",
                    edge.from, edge.to, edge.from
                ));
            }
        }
        if self.node(&self.start).expect("checked").kind == Kind::Join {
            return Err("the start is not a join".into());
        }
        let mut reached = BTreeSet::from([self.start.as_str()]);
        let mut frontier = vec![self.start.as_str()];
        while let Some(at) = frontier.pop() {
            for edge in self.edges.iter().filter(|e| e.from == at) {
                if reached.insert(edge.to.as_str()) {
                    frontier.push(edge.to.as_str());
                }
            }
        }
        if let Some(lost) = self.nodes.iter().find(|n| !reached.contains(n.id.as_str())) {
            return Err(format!("{} cannot be reached from {}", lost.id, self.start));
        }
        Ok(())
    }

    /// The nodes from which `to` can be reached (not `to` itself, unless
    /// through a loop).
    fn leading_to(&self, to: &str) -> BTreeSet<String> {
        let mut found = BTreeSet::new();
        let mut frontier = vec![to.to_string()];
        while let Some(at) = frontier.pop() {
            for edge in self.edges.iter().filter(|e| e.to == at) {
                if found.insert(edge.from.clone()) {
                    frontier.push(edge.from.clone());
                }
            }
        }
        found
    }
}

/// What a node's run answered.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Answer {
    /// Its text.
    pub text: String,
    /// Its JSON, for a node whose answer keeps to a schema.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub json: Option<Value>,
    /// The session it ran in, if it kept one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// The tokens it used.
    #[serde(default)]
    pub tokens: u64,
    /// Whether its session read content from outside, which may steer what
    /// it says: the nodes it reaches run as content from outside.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub outside: bool,
}

/// A node to run: which, with what reached it, and its item of a fan-out.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Activation {
    /// The node.
    pub node: String,
    /// The answers that reached it: from which node, and what.
    pub inputs: Vec<(String, String)>,
    /// Its item, if a fan-out started it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<Value>,
    /// Whether something that reached it came from a session that read
    /// content from outside: its own session starts tainted.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub outside: bool,
}

/// A node that ran.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ran {
    /// What ran.
    pub activation: Activation,
    /// What it answered, or why it failed.
    pub answer: Result<Answer, String>,
}

/// Runs a node: the session that the node is, on `prompt`.
pub trait Executor: Sync {
    /// Runs `node` on `prompt`: what it is asked, the task, and what
    /// reached it. A node with an `output` schema answers JSON. With
    /// `outside`, what reached it came from a session that read content
    /// from outside, and its session is to be tainted from the start.
    fn run(&self, node: &Node, prompt: &str, outside: bool) -> Result<Answer, String>;

    /// Told, before a wave runs, how many nodes run in it together (to
    /// share a budget among them). By default, nothing is done.
    fn wave(&self, _size: usize) {}
}

/// How a graph's run stands.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Standing {
    /// Nodes are still to run.
    Going,
    /// It finished: the answer of its end.
    Done(Answer),
    /// It stopped: why.
    Stopped(String),
}

/// A graph's run, as it is kept between waves.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GraphRun {
    /// The task.
    pub task: String,
    /// What is to run next.
    pub queue: Vec<Activation>,
    /// What ran, in order.
    pub ran: Vec<Ran>,
    /// How it stands.
    pub standing: Standing,
}

impl GraphRun {
    /// A run of `graph` on `task`, its start to run first.
    pub fn start(graph: &Graph, task: &str) -> GraphRun {
        GraphRun {
            task: task.to_string(),
            queue: vec![Activation {
                node: graph.start.clone(),
                inputs: Vec::new(),
                item: None,
                outside: false,
            }],
            ran: Vec::new(),
            standing: Standing::Going,
        }
    }

    /// How many times `node` ran.
    pub fn visits(&self, node: &str) -> u32 {
        self.ran
            .iter()
            .filter(|r| r.activation.node == node)
            .count() as u32
    }

    /// Runs the next wave: every activation that may run now (a join only
    /// once nothing that leads to it is left to run), together, through
    /// `executor`; then what their answers lead to. Returns how the run
    /// stands.
    pub fn step(&mut self, graph: &Graph, executor: &dyn Executor) -> &Standing {
        if self.standing != Standing::Going {
            return &self.standing;
        }
        if self.queue.is_empty() {
            self.standing = self.finish(graph);
            return &self.standing;
        }
        let waiting: BTreeSet<String> = self.queue.iter().map(|a| a.node.clone()).collect();
        let (wave, rest): (Vec<Activation>, Vec<Activation>) =
            self.queue.drain(..).partition(|a| {
                let node = graph.node(&a.node).expect("checked");
                node.kind != Kind::Join || {
                    let upstream = graph.leading_to(&a.node);
                    waiting
                        .iter()
                        .all(|w| w == &a.node || !upstream.contains(w))
                }
            });
        self.queue = rest;
        if wave.is_empty() {
            self.standing = Standing::Stopped("the joins wait on each other".into());
            return &self.standing;
        }
        // Limits first: a wave that would go past them does not run.
        if self.ran.len() + wave.len() > graph.max_runs as usize {
            self.standing = Standing::Stopped(format!(
                "the graph would run more than {} nodes",
                graph.max_runs
            ));
            return &self.standing;
        }
        for activation in &wave {
            let node = graph.node(&activation.node).expect("checked");
            let earlier = self.visits(&node.id)
                + wave
                    .iter()
                    .take_while(|a| !std::ptr::eq(*a, activation))
                    .filter(|a| a.node == node.id)
                    .count() as u32;
            if earlier >= node.max_visits {
                self.standing = Standing::Stopped(format!(
                    "{} would run more than {} times",
                    node.id, node.max_visits
                ));
                return &self.standing;
            }
        }
        executor.wave(wave.len());
        let task = self.task.clone();
        let answers: Vec<Result<Answer, String>> = std::thread::scope(|scope| {
            let handles: Vec<_> = wave
                .iter()
                .map(|activation| {
                    let node = graph.node(&activation.node).expect("checked");
                    let prompt = prompt_for(node, &task, activation);
                    let outside = activation.outside;
                    scope.spawn(move || {
                        executor
                            .run(node, &prompt, outside)
                            .and_then(|a| kept(node, a))
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| {
                    h.join()
                        .unwrap_or_else(|_| Err("the node's run failed".into()))
                })
                .collect()
        });
        for (activation, answer) in wave.into_iter().zip(answers) {
            if let Ok(answer) = &answer {
                let next = next_of(graph, &activation.node, answer);
                for (to, item) in next {
                    let from = (activation.node.clone(), answer.text.clone());
                    let to_node = graph.node(&to).expect("checked");
                    match self
                        .queue
                        .iter_mut()
                        .find(|a| a.node == to && to_node.kind == Kind::Join)
                    {
                        Some(join) => {
                            join.inputs.push(from);
                            join.outside |= answer.outside;
                        }
                        None => self.queue.push(Activation {
                            node: to,
                            inputs: vec![from],
                            item,
                            outside: answer.outside,
                        }),
                    }
                }
            }
            let failed = answer.as_ref().err().cloned();
            self.ran.push(Ran { activation, answer });
            if let Some(why) = failed {
                let node = &self.ran.last().expect("pushed").activation.node;
                self.standing = Standing::Stopped(format!("{node} failed: {why}"));
                return &self.standing;
            }
        }
        if self.queue.is_empty() {
            self.standing = self.finish(graph);
        }
        &self.standing
    }

    /// Runs the graph until it is done or stops, calling `kept` after each
    /// wave (to keep the run on disk).
    pub fn run(
        &mut self,
        graph: &Graph,
        executor: &dyn Executor,
        kept: &mut dyn FnMut(&GraphRun),
    ) -> Standing {
        while self.standing == Standing::Going {
            self.step(graph, executor);
            kept(self);
        }
        self.standing.clone()
    }

    /// The answer of a run with nothing left to run: its end's last, or the
    /// last node's.
    fn finish(&self, graph: &Graph) -> Standing {
        let answers = self.ran.iter().rev().filter_map(|r| {
            r.answer
                .as_ref()
                .ok()
                .map(|answer| (&r.activation.node, answer))
        });
        let found = match &graph.end {
            Some(end) => answers
                .into_iter()
                .find(|(node, _)| *node == end)
                .map(|(_, a)| a.clone()),
            None => answers.into_iter().next().map(|(_, a)| a.clone()),
        };
        match found {
            Some(answer) => Standing::Done(answer),
            None => Standing::Stopped(match &graph.end {
                Some(end) => format!("{end} never ran"),
                None => "nothing ran".into(),
            }),
        }
    }
}

/// What a node is told: its instructions, the task, its item, what reached
/// it, and the schema its answer keeps to.
pub fn prompt_for(node: &Node, task: &str, activation: &Activation) -> String {
    let mut prompt = format!("{}\n\nThe task:\n{task}", node.instructions.trim());
    if let Some(item) = &activation.item {
        let item = item
            .as_str()
            .map_or_else(|| item.to_string(), str::to_string);
        prompt.push_str(&format!("\n\nYour part of it:\n{item}"));
    }
    for (from, text) in &activation.inputs {
        prompt.push_str(&format!("\n\nFrom {from}:\n{}", text.trim()));
    }
    if let Some(schema) = &node.output {
        prompt.push_str(&format!(
            "\n\nAnswer with one JSON object that keeps to this schema, and nothing else:\n{schema}"
        ));
    }
    prompt
}

/// A node's answer, its JSON found and checked where it keeps to a schema.
fn kept(node: &Node, mut answer: Answer) -> Result<Answer, String> {
    let Some(schema) = &node.output else {
        return Ok(answer);
    };
    let json = match answer.json.clone() {
        Some(json) => json,
        None => crate::quarantine::answer_of(&answer.text)
            .map_err(|_| "its answer is not a JSON object".to_string())?,
    };
    // The fields a schema requires are there, and `pass` is a boolean.
    for name in schema["required"].as_array().into_iter().flatten() {
        let name = name.as_str().unwrap_or_default();
        if json.get(name).is_none() {
            return Err(format!("its answer leaves out {name}"));
        }
    }
    if json.get("pass").is_some_and(|p| !p.is_boolean()) {
        return Err("its answer's pass is not true or false".into());
    }
    answer.json = Some(json);
    Ok(answer)
}

/// Where a node's answer leads: each node an edge reaches, and its item for
/// a fan-out.
fn next_of(graph: &Graph, from: &str, answer: &Answer) -> Vec<(String, Option<Value>)> {
    let json = answer.json.as_ref();
    let field = |name: &str| json.and_then(|j| j.get(name));
    let mut next = Vec::new();
    for edge in graph.edges.iter().filter(|e| e.from == from) {
        match &edge.when {
            When::Always => next.push((edge.to.clone(), None)),
            When::Pass if field("pass") == Some(&json!(true)) => next.push((edge.to.clone(), None)),
            When::Fail if field("pass") == Some(&json!(false)) => {
                next.push((edge.to.clone(), None))
            }
            When::Field {
                field: name,
                equals,
            } if field(name) == Some(equals) => next.push((edge.to.clone(), None)),
            When::Each { field: name } => {
                for item in field(name)
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .take(MAX_FAN_OUT)
                {
                    next.push((edge.to.clone(), Some(item.clone())));
                }
            }
            _ => {}
        }
    }
    next
}

/// The graphs a router may propose, by name.
pub const TEMPLATES: [&str; 3] = ["fan-out", "work-check", "draft-review"];

/// A graph the runtime knows: `fan-out` (a task split into parts that need
/// no one else's work, each done by an agent of its own, then put
/// together), `work-check` (the work, then a check by an agent that did not
/// do it, back to the work until it passes) or `draft-review` (a draft, a
/// review, and the draft revised until the review passes).
pub fn template(name: &str) -> Option<Graph> {
    let check = json!({
        "type": "object",
        "properties": {"pass": {"type": "boolean"}, "why": {"type": "string"}},
        "required": ["pass", "why"]
    });
    let node = |id: &str, kind: Kind, instructions: &str, output: Option<Value>| Node {
        id: id.into(),
        kind,
        instructions: instructions.into(),
        tools: None,
        model: None,
        output,
        max_visits: default_max_visits(),
    };
    let edge = |from: &str, to: &str, when: When| Edge {
        from: from.into(),
        to: to.into(),
        when,
    };
    let graph = match name {
        "fan-out" => Graph {
            name: name.into(),
            start: "split".into(),
            end: Some("combine".into()),
            nodes: vec![
                node(
                    "split",
                    Kind::Agent,
                    "Split the task into parts that can be done apart, each without the \
                     others' results: two to six of them, each said in full. Do none of them \
                     yourself.",
                    Some(json!({
                        "type": "object",
                        "properties": {"parts": {"type": "array", "items": {"type": "string"}}},
                        "required": ["parts"]
                    })),
                ),
                Node {
                    max_visits: MAX_FAN_OUT as u32,
                    ..node(
                        "part",
                        Kind::Agent,
                        "Do your part of the task, and only that part, and report what you \
                         found or did.",
                        None,
                    )
                },
                node(
                    "combine",
                    Kind::Join,
                    "Put the parts' reports together into one answer to the task: what they \
                     found, where they disagree, and what is missing.",
                    None,
                ),
            ],
            edges: vec![
                edge(
                    "split",
                    "part",
                    When::Each {
                        field: "parts".into(),
                    },
                ),
                edge("part", "combine", When::Always),
            ],
            max_runs: default_max_runs(),
        },
        "work-check" => Graph {
            name: name.into(),
            start: "work".into(),
            end: Some("work".into()),
            nodes: vec![
                node(
                    "work",
                    Kind::Agent,
                    "Do the task. If a check of an earlier attempt came back, fix what it \
                     found.",
                    None,
                ),
                node(
                    "check",
                    Kind::Agent,
                    "Check the work against the task, as someone who did not do it: run what \
                     can be run, read what was written. Pass it only if it does what the task \
                     asks; say why.",
                    Some(check.clone()),
                ),
            ],
            edges: vec![
                edge("work", "check", When::Always),
                edge("check", "work", When::Fail),
            ],
            max_runs: default_max_runs(),
        },
        "draft-review" => Graph {
            name: name.into(),
            start: "draft".into(),
            end: Some("draft".into()),
            nodes: vec![
                node(
                    "draft",
                    Kind::Agent,
                    "Write what the task asks for. If a review of an earlier draft came back, \
                     revise the draft as it says.",
                    None,
                ),
                node(
                    "review",
                    Kind::Agent,
                    "Review the draft against the task, as an editor who did not write it. \
                     Pass it only if it is ready; otherwise say what to change.",
                    Some(check),
                ),
            ],
            edges: vec![
                edge("draft", "review", When::Always),
                edge("review", "draft", When::Fail),
            ],
            max_runs: default_max_runs(),
        },
        _ => return None,
    };
    Some(graph)
}

/// What the router is told: to keep a task to one agent unless it divides.
pub const ROUTER: &str = "You decide how a task is worked on. One agent with good tools is best \
     for most tasks, and costs least: choose `single` unless the task clearly divides. Choose \
     `fan-out` only for a task of several parts that each need none of the others' results (look \
     up three things, review four files); `work-check` for work whose result can be checked by \
     someone who did not do it (code with tests, figures to recompute); `draft-review` for \
     writing that benefits from an editor. Answer with one JSON object: {\"shape\": \"single\" | \
     \"fan-out\" | \"work-check\" | \"draft-review\", \"why\": \"…\"}.";

/// How a router said to work on a task.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Route {
    /// `single`, or a template's name.
    pub shape: String,
    /// Why.
    pub why: String,
}

/// What the router is told when it may choose among one agent and
/// `shapes` alone ([`ROUTER`] when it may choose any template).
pub fn router_for(shapes: &[&str]) -> String {
    if TEMPLATES.iter().all(|t| shapes.contains(t)) {
        return ROUTER.into();
    }
    let mut text = String::from(
        "You decide how a task is worked on. One agent with good tools is best for most tasks, \
         and costs least: choose `single` unless the task clearly divides.",
    );
    for shape in shapes {
        text.push(' ');
        text.push_str(match *shape {
            "fan-out" => {
                "Choose `fan-out` only for a task of several parts that each need none of the \
                 others' results (look up three things, review four files)."
            }
            "work-check" => {
                "Choose `work-check` for work whose result can be checked by someone who did \
                 not do it (code with tests, figures to recompute)."
            }
            "draft-review" => "Choose `draft-review` for writing that benefits from an editor.",
            _ => "",
        });
    }
    let options: Vec<String> = std::iter::once("single")
        .chain(shapes.iter().copied())
        .map(|s| format!("\"{s}\""))
        .collect();
    text.push_str(&format!(
        " Answer with one JSON object: {{\"shape\": {}, \"why\": \"…\"}}.",
        options.join(" | ")
    ));
    text
}

/// The route in what a router wrote: one of the shapes it may give, or
/// `single` if it gave none it may.
pub fn route_of(text: &str) -> Route {
    route_among_of(text, &TEMPLATES)
}

/// The route in what a router wrote, among one agent and `shapes`:
/// `single` if it gave none of them.
pub fn route_among_of(text: &str, shapes: &[&str]) -> Route {
    let parsed = crate::quarantine::answer_of(text).ok();
    let shape = parsed
        .as_ref()
        .and_then(|j| j["shape"].as_str())
        .filter(|s| *s == "single" || shapes.contains(s))
        .unwrap_or("single")
        .to_string();
    let why = parsed
        .as_ref()
        .and_then(|j| j["why"].as_str())
        .unwrap_or("the router gave no shape it may: one agent")
        .to_string();
    Route { shape, why }
}

/// A summary of a run, a node per line, for what is shown of it.
pub fn summary(run: &GraphRun) -> Vec<String> {
    let mut lines = Vec::new();
    for ran in &run.ran {
        let item = ran
            .activation
            .item
            .as_ref()
            .map(|i| {
                format!(
                    " [{}]",
                    i.as_str().map_or_else(|| i.to_string(), str::to_string)
                )
            })
            .unwrap_or_default();
        lines.push(match &ran.answer {
            Ok(answer) => {
                let first = answer.text.lines().next().unwrap_or_default();
                let short: String = first.chars().take(120).collect();
                let session = answer
                    .session
                    .as_ref()
                    .map(|s| format!(" (session {s})"))
                    .unwrap_or_default();
                format!("{}{item}{session}: {short}", ran.activation.node)
            }
            Err(why) => format!("{}{item}: failed: {why}", ran.activation.node),
        });
    }
    lines
}

/// A graph in a line: each edge, from one node to the next, when it is
/// followed, and which nodes are joins.
pub fn outline(graph: &Graph) -> String {
    let edges: Vec<String> = graph
        .edges
        .iter()
        .map(|edge| {
            let to = match graph.node(&edge.to).map(|n| n.kind) {
                Some(Kind::Join) => format!("{} (waits for all)", edge.to),
                _ => edge.to.clone(),
            };
            let from = &edge.from;
            match &edge.when {
                When::Always => format!("{from} → {to}"),
                When::Pass => format!("{from} → {to} if it passes"),
                When::Fail => format!("{from} → {to} if it fails"),
                When::Field { field, equals } => format!("{from} → {to} if {field} is {equals}"),
                When::Each { field } => format!("{from} → {to} for each of its {field}"),
            }
        })
        .collect();
    edges.join("; ")
}

/// The graphs' nodes by id, for looking them up while a run is kept.
pub fn nodes_by_id(graph: &Graph) -> BTreeMap<&str, &Node> {
    graph.nodes.iter().map(|n| (n.id.as_str(), n)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Answers each node from a script: by node, in turn. An answer that
    /// begins with `outside:` is one from a session that read content from
    /// outside.
    struct Script {
        answers: Mutex<BTreeMap<String, Vec<String>>>,
        prompts: Mutex<Vec<(String, String)>>,
        outside: Mutex<Vec<(String, bool)>>,
    }

    impl Script {
        fn new(answers: &[(&str, &[&str])]) -> Script {
            Script {
                answers: Mutex::new(
                    answers
                        .iter()
                        .map(|(n, a)| (n.to_string(), a.iter().map(|s| s.to_string()).collect()))
                        .collect(),
                ),
                prompts: Mutex::new(Vec::new()),
                outside: Mutex::new(Vec::new()),
            }
        }
    }

    impl Executor for Script {
        fn run(&self, node: &Node, prompt: &str, outside: bool) -> Result<Answer, String> {
            self.prompts
                .lock()
                .unwrap()
                .push((node.id.clone(), prompt.to_string()));
            self.outside
                .lock()
                .unwrap()
                .push((node.id.clone(), outside));
            let mut answers = self.answers.lock().unwrap();
            let queue = answers.get_mut(&node.id).ok_or("no answer")?;
            if queue.is_empty() {
                return Err("no more answers".into());
            }
            let text = queue.remove(0);
            let (text, read) = match text.strip_prefix("outside:") {
                Some(text) => (text.to_string(), true),
                None => (text, false),
            };
            Ok(Answer {
                text,
                json: None,
                session: None,
                tokens: 10,
                outside: read,
            })
        }
    }

    #[test]
    fn the_templates_check() {
        for name in TEMPLATES {
            template(name).unwrap().check().unwrap();
        }
        assert!(template("swarm").is_none());
        assert_eq!(
            outline(&template("fan-out").unwrap()),
            "split → part for each of its parts; part → combine (waits for all)"
        );
        assert_eq!(
            outline(&template("work-check").unwrap()),
            "work → check; check → work if it fails"
        );
    }

    #[test]
    fn a_bad_graph_is_refused() {
        let mut graph = template("work-check").unwrap();
        graph.edges.push(Edge {
            from: "work".into(),
            to: "nowhere".into(),
            when: When::Always,
        });
        assert!(graph.check().unwrap_err().contains("nowhere"));
        let mut graph = template("work-check").unwrap();
        graph.nodes[0].output = None;
        graph.edges[0].when = When::Pass;
        assert!(graph.check().unwrap_err().contains("keeps to no schema"));
        let mut graph = template("fan-out").unwrap();
        graph.edges.clear();
        assert!(graph.check().unwrap_err().contains("cannot be reached"));
    }

    #[test]
    fn a_fan_out_runs_its_parts_together_and_joins_them() {
        let graph = template("fan-out").unwrap();
        let script = Script::new(&[
            (
                "split",
                &[r#"{"parts": ["the price", "the licence", "the support"]}"#],
            ),
            ("part", &["12 USD", "MIT", "email only"]),
            ("combine", &["12 USD, MIT, support by email."]),
        ]);
        let mut run = GraphRun::start(&graph, "Look the product up.");
        let mut waves = 0;
        let standing = run.run(&graph, &script, &mut |_| waves += 1);
        assert_eq!(
            standing,
            Standing::Done(Answer {
                text: "12 USD, MIT, support by email.".into(),
                json: None,
                session: None,
                tokens: 10,
                outside: false,
            })
        );
        // The split, the three parts at once, the join.
        assert_eq!(waves, 3);
        assert_eq!(run.visits("part"), 3);
        let prompts = script.prompts.lock().unwrap();
        let combine = &prompts.iter().find(|(n, _)| n == "combine").unwrap().1;
        assert_eq!(combine.matches("From part:").count(), 3);
        // The parts ran side by side, in any order: each was told its own.
        for item in ["the price", "the licence", "the support"] {
            assert!(
                prompts
                    .iter()
                    .any(|(n, p)| n == "part" && p.contains(&format!("Your part of it:\n{item}"))),
                "no part was told {item}"
            );
        }
        // A run kept on disk comes back as it was.
        let kept: GraphRun = serde_json::from_str(&serde_json::to_string(&run).unwrap()).unwrap();
        assert_eq!(kept, run);
    }

    #[test]
    fn a_failed_check_loops_back_until_it_passes_or_the_visits_run_out() {
        let graph = template("work-check").unwrap();
        let script = Script::new(&[
            ("work", &["first try", "second try"]),
            (
                "check",
                &[
                    r#"{"pass": false, "why": "the tests fail"}"#,
                    r#"{"pass": true, "why": "the tests pass"}"#,
                ],
            ),
        ]);
        let mut run = GraphRun::start(&graph, "Fix the bug.");
        let standing = run.run(&graph, &script, &mut |_| {});
        let Standing::Done(answer) = standing else {
            panic!("{standing:?}");
        };
        assert_eq!(answer.text, "second try");
        let prompts = script.prompts.lock().unwrap();
        assert!(prompts[2].1.contains("From check:\n{\"pass\": false"));

        // A check that never passes stops at the work's third visit.
        let script = Script::new(&[
            ("work", &["a", "b", "c", "d"]),
            ("check", &[r#"{"pass": false, "why": "no"}"#; 4]),
        ]);
        let mut run = GraphRun::start(&graph, "Fix the bug.");
        assert_eq!(
            run.run(&graph, &script, &mut |_| {}),
            Standing::Stopped("work would run more than 3 times".into())
        );
    }

    #[test]
    fn a_node_whose_answer_breaks_its_schema_stops_the_run() {
        let graph = template("work-check").unwrap();
        let script = Script::new(&[("work", &["done"]), ("check", &["looks fine to me"])]);
        let mut run = GraphRun::start(&graph, "Fix the bug.");
        let Standing::Stopped(why) = run.run(&graph, &script, &mut |_| {}) else {
            panic!("expected a stop");
        };
        assert!(why.starts_with("check failed"), "{why}");
    }

    #[test]
    fn what_a_tainted_node_says_taints_the_nodes_it_reaches() {
        let graph = template("fan-out").unwrap();
        let script = Script::new(&[
            ("split", &[r#"{"parts": ["the price", "the licence"]}"#]),
            // The first part read a page from outside; the second did not.
            ("part", &["outside:12 USD", "MIT"]),
            ("combine", &["12 USD, MIT."]),
        ]);
        let mut run = GraphRun::start(&graph, "Look the product up.");
        assert!(matches!(
            run.run(&graph, &script, &mut |_| {}),
            Standing::Done(_)
        ));
        let outside = script.outside.lock().unwrap();
        let of = |node: &str| -> Vec<bool> {
            outside
                .iter()
                .filter(|(n, _)| n == node)
                .map(|(_, o)| *o)
                .collect()
        };
        assert_eq!(of("split"), [false]);
        assert_eq!(of("part"), [false, false]);
        // The join took in what the tainted part said.
        assert_eq!(of("combine"), [true]);
    }

    #[test]
    fn a_router_told_fewer_shapes_keeps_to_them() {
        let text = router_for(&["fan-out"]);
        assert!(text.starts_with("You decide how a task is worked on."));
        assert!(
            text.contains("`fan-out`") && !text.contains("`work-check`"),
            "{text}"
        );
        assert!(
            text.ends_with(r#"{"shape": "single" | "fan-out", "why": "…"}."#),
            "{text}"
        );
        assert_eq!(router_for(&TEMPLATES), ROUTER);
        let said = r#"{"shape": "work-check", "why": "tests to run"}"#;
        assert_eq!(route_among_of(said, &["fan-out"]).shape, "single");
        assert_eq!(route_among_of(said, &["work-check"]).shape, "work-check");
    }

    #[test]
    fn the_router_keeps_to_one_agent_unless_told_a_shape_it_may_give() {
        assert_eq!(
            route_of(r#"{"shape": "fan-out", "why": "three lookups"}"#).shape,
            "fan-out"
        );
        assert_eq!(
            route_of(r#"{"shape": "swarm", "why": "more is better"}"#).shape,
            "single"
        );
        assert_eq!(route_of("I think a team would help.").shape, "single");
    }
}
