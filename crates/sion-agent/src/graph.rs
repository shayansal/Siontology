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
//!
//! A join waits for everything that leads to it ([`Wait::All`]), for the
//! first answer, or for a quorum; a [`Kind::Vote`] is a join that answers,
//! without a model, with the answer that reached it most often. A node may
//! run in several copies side by side ([`Node::copies`]), and with only
//! the tools that read ([`Node::read_only`]). A run is pinned to its
//! graph's digest ([`digest`]), can be replayed from what it recorded
//! ([`replay`]) to check that the graph still routes it the same way, and
//! goes on after a node that waited for a person ([`GraphRun::resume`]).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sion_core::hash::{hash_domain, Hash32};

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
    /// An agent's session that waits for what leads to it, as its
    /// [`Wait`] says, and runs once with every answer that reached it.
    Join,
    /// A join that runs no session: its answer is the one that reached it
    /// most often, case and spacing aside (a tie goes to the earliest).
    Vote,
}

impl Kind {
    /// Whether it gathers the answers that reach it into one run.
    pub fn joins(self) -> bool {
        matches!(self, Kind::Join | Kind::Vote)
    }
}

/// When a join runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Wait {
    /// Once nothing that leads to it is left to run.
    #[default]
    All,
    /// As soon as an answer has reached it, with every answer that reached
    /// it in the same wave; what reaches it later is not passed on.
    First,
    /// As soon as this many answers have reached it, or nothing that leads
    /// to it is left to run; what reaches it later is not passed on.
    Quorum(u32),
}

impl Wait {
    fn is_all(&self) -> bool {
        *self == Wait::All
    }
}

fn one() -> u32 {
    1
}

fn is_one(n: &u32) -> bool {
    *n == 1
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
    /// How many copies of it run side by side each time it is reached,
    /// each told which attempt it is (for a vote).
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub copies: u32,
    /// Whether it may use only the tools that read.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub read_only: bool,
    /// When it runs, if it is a join.
    #[serde(default, skip_serializing_if = "Wait::is_all")]
    pub wait: Wait,
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
            if node.copies == 0 || node.copies > node.max_visits {
                return Err(format!(
                    "{}: copies is 1 to its max_visits ({})",
                    node.id, node.max_visits
                ));
            }
            if node.copies > 1 && node.kind.joins() {
                return Err(format!("{}: a join runs in one copy", node.id));
            }
            if !node.kind.joins() && !node.wait.is_all() {
                return Err(format!("{}: only a join waits", node.id));
            }
            if node.wait == Wait::Quorum(0) {
                return Err(format!("{}: a quorum is at least 1", node.id));
            }
            if node.kind == Kind::Vote && node.output.is_some() {
                return Err(format!(
                    "{}: a vote answers with what reached it, and keeps to no schema",
                    node.id
                ));
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
        if self.node(&self.start).expect("checked").kind.joins() {
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
    /// Which copy it is, and of how many, for a node run in copies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<(u32, u32)>,
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
    /// The joins that waited for the first answer or a quorum and ran: what
    /// reaches them later is not passed on.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub closed: BTreeSet<String>,
}

impl GraphRun {
    /// A run of `graph` on `task`, its start to run first (in its copies).
    pub fn start(graph: &Graph, task: &str) -> GraphRun {
        let start = graph.node(&graph.start).map_or(1, |n| n.copies.max(1));
        GraphRun {
            task: task.to_string(),
            queue: (1..=start)
                .map(|k| Activation {
                    node: graph.start.clone(),
                    inputs: Vec::new(),
                    item: None,
                    outside: false,
                    attempt: (start > 1).then_some((k, start)),
                })
                .collect(),
            ran: Vec::new(),
            standing: Standing::Going,
            closed: BTreeSet::new(),
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
                if !node.kind.joins() {
                    return true;
                }
                let upstream = graph.leading_to(&a.node);
                let alone = waiting
                    .iter()
                    .all(|w| w == &a.node || !upstream.contains(w));
                match node.wait {
                    Wait::All => alone,
                    Wait::First => !a.inputs.is_empty(),
                    Wait::Quorum(n) => a.inputs.len() >= n as usize || alone,
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
        for activation in &wave {
            let node = graph.node(&activation.node).expect("checked");
            if node.kind.joins() && !node.wait.is_all() {
                self.closed.insert(node.id.clone());
            }
        }
        let sessions = wave
            .iter()
            .filter(|a| graph.node(&a.node).is_some_and(|n| n.kind != Kind::Vote))
            .count();
        executor.wave(sessions);
        let task = self.task.clone();
        let answers: Vec<Result<Answer, String>> = std::thread::scope(|scope| {
            let started: Vec<Started<'_>> = wave
                .iter()
                .map(|activation| {
                    let node = graph.node(&activation.node).expect("checked");
                    if node.kind == Kind::Vote {
                        return Started::Done(voted(activation));
                    }
                    let prompt = prompt_for(node, &task, activation);
                    let outside = activation.outside;
                    Started::Running(scope.spawn(move || {
                        executor
                            .run(node, &prompt, outside)
                            .and_then(|a| kept(node, a))
                    }))
                })
                .collect();
            started
                .into_iter()
                .map(|s| match s {
                    Started::Running(h) => h
                        .join()
                        .unwrap_or_else(|_| Err("the node's run failed".into())),
                    Started::Done(answer) => answer,
                })
                .collect()
        });
        // Every answer of the wave is kept, and what the answers lead to is
        // queued, before a failure stops the run.
        let mut failed = None;
        for (activation, answer) in wave.into_iter().zip(answers) {
            match &answer {
                Ok(answer) => self.route(graph, &activation, answer),
                Err(why) => {
                    failed.get_or_insert_with(|| format!("{} failed: {why}", activation.node));
                }
            }
            self.ran.push(Ran { activation, answer });
        }
        if let Some(why) = failed {
            self.standing = Standing::Stopped(why);
            return &self.standing;
        }
        if self.queue.is_empty() {
            self.standing = self.finish(graph);
        }
        &self.standing
    }

    /// Queues what `answer`, from `from`'s run, leads to: a join's input
    /// added to it if it waits in the queue (and dropped if it has closed),
    /// a node run in copies queued in each.
    fn route(&mut self, graph: &Graph, from: &Activation, answer: &Answer) {
        for (to, item) in next_of(graph, &from.node, answer) {
            if self.closed.contains(&to) {
                continue;
            }
            let input = (from.node.clone(), answer.text.clone());
            let to_node = graph.node(&to).expect("checked");
            if to_node.kind.joins() {
                if let Some(join) = self.queue.iter_mut().find(|a| a.node == to) {
                    join.inputs.push(input);
                    join.outside |= answer.outside;
                    continue;
                }
            }
            let copies = to_node.copies.max(1);
            for k in 1..=copies {
                self.queue.push(Activation {
                    node: to.clone(),
                    inputs: vec![input.clone()],
                    item: item.clone(),
                    outside: answer.outside,
                    attempt: (copies > 1).then_some((k, copies)),
                });
            }
        }
    }

    /// Goes on after the node run `index`, which failed (its session waited
    /// for a person's approval, say), now that it has `answer`: what the
    /// answer leads to is queued, and the run goes on once no node run has
    /// failed.
    pub fn resume(&mut self, graph: &Graph, index: usize, answer: Answer) -> Result<(), String> {
        if !matches!(self.standing, Standing::Stopped(_)) {
            return Err("the run did not stop".into());
        }
        let ran = self.ran.get(index).ok_or("no such node run")?;
        if ran.answer.is_ok() {
            return Err(format!("{} did not fail", ran.activation.node));
        }
        let node = graph.node(&ran.activation.node).ok_or("no such node")?;
        let answer = kept(node, answer)?;
        let activation = ran.activation.clone();
        self.route(graph, &activation, &answer);
        self.ran[index].answer = Ok(answer);
        self.standing = match self.ran.iter().find_map(|r| {
            r.answer
                .as_ref()
                .err()
                .map(|why| format!("{} failed: {why}", r.activation.node))
        }) {
            Some(why) => Standing::Stopped(why),
            None if self.queue.is_empty() => self.finish(graph),
            None => Standing::Going,
        };
        Ok(())
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

/// A node of a wave: running in a thread, or answered already (a vote).
enum Started<'scope> {
    Running(std::thread::ScopedJoinHandle<'scope, Result<Answer, String>>),
    Done(Result<Answer, String>),
}

/// The answer given most often, case and spacing aside, a tie going to the
/// earliest: its first wording, its votes, of how many answers.
pub fn vote(answers: &[String]) -> Option<(String, usize, usize)> {
    let key = |text: &str| {
        text.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
            .trim_end_matches('.')
            .to_string()
    };
    let mut counts: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for (i, answer) in answers.iter().enumerate() {
        let entry = counts.entry(key(answer)).or_insert((0, i));
        entry.0 += 1;
    }
    counts
        .values()
        .max_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)))
        .map(|&(votes, first)| (answers[first].trim().to_string(), votes, answers.len()))
}

/// A vote node's answer: the most common of what reached it, with its
/// count (`{"votes": …, "of": …}`).
fn voted(activation: &Activation) -> Result<Answer, String> {
    let answers: Vec<String> = activation.inputs.iter().map(|(_, t)| t.clone()).collect();
    let (text, votes, of) = vote(&answers).ok_or("nothing reached the vote")?;
    Ok(Answer {
        text,
        json: Some(json!({"votes": votes, "of": of})),
        session: None,
        tokens: 0,
        outside: activation.outside,
    })
}

/// The digest a run of `graph` is pinned to: of its definition, as it
/// serialises.
pub fn digest(graph: &Graph) -> Hash32 {
    hash_domain(
        "sion-agent/graph/v1",
        &serde_json::to_vec(graph).unwrap_or_default(),
    )
}

/// Answers each node of a replay from what a run recorded: the first node
/// run not yet used of the same node and the same prompt.
struct Recorded<'a> {
    run: &'a GraphRun,
    used: Mutex<Vec<bool>>,
}

impl Executor for Recorded<'_> {
    fn run(&self, node: &Node, prompt: &str, _outside: bool) -> Result<Answer, String> {
        let mut used = self.used.lock().unwrap_or_else(|e| e.into_inner());
        let found = self.run.ran.iter().enumerate().find(|(i, ran)| {
            !used[*i]
                && ran.activation.node == node.id
                && prompt_for(node, &self.run.task, &ran.activation) == prompt
        });
        match found {
            Some((i, ran)) => {
                used[i] = true;
                ran.answer.clone()
            }
            None => Err(format!(
                "the run recorded no answer to what {} is asked now",
                node.id
            )),
        }
    }
}

/// What a replay came to: whether the graph routed the recorded answers as
/// the run did, and if not, where it went another way.
#[derive(Clone, Debug, PartialEq)]
pub struct Replayed {
    /// The run the replay made.
    pub run: GraphRun,
    /// The first node run at which it differs from the recorded one, and
    /// how; none if it ran the same.
    pub differs: Option<(usize, String)>,
}

/// Replays `recorded` (a run that finished or stopped) through `graph`,
/// each node answered with what the run recorded for it: the graph, as it
/// is now, must ask each node what it asked then and route the answers the
/// same way, to the same end.
pub fn replay(graph: &Graph, recorded: &GraphRun) -> Result<Replayed, String> {
    if recorded.standing == Standing::Going {
        return Err("the run has not finished: go on with it first".into());
    }
    graph.check()?;
    let executor = Recorded {
        run: recorded,
        used: Mutex::new(vec![false; recorded.ran.len()]),
    };
    let mut run = GraphRun::start(graph, &recorded.task);
    while run.standing == Standing::Going && run.ran.len() <= recorded.ran.len() {
        run.step(graph, &executor);
    }
    let differs = (0..run.ran.len().max(recorded.ran.len()))
        .find(|&i| run.ran.get(i) != recorded.ran.get(i))
        .map(|i| {
            let what = match (run.ran.get(i), recorded.ran.get(i)) {
                (Some(now), Some(then)) if now.activation.node != then.activation.node => {
                    format!(
                        "the graph ran {} where the run had run {}",
                        now.activation.node, then.activation.node
                    )
                }
                (Some(now), Some(then)) if now.activation != then.activation => format!(
                    "{} was reached with other inputs than in the run",
                    now.activation.node
                ),
                (Some(now), Some(_)) => match &now.answer {
                    Err(why) => why.clone(),
                    Ok(_) => format!("{} answered otherwise", now.activation.node),
                },
                (Some(now), None) => format!(
                    "the graph ran {} after the run had ended",
                    now.activation.node
                ),
                (None, Some(then)) => format!("the graph ended before {}", then.activation.node),
                (None, None) => unreachable!("one of them ran it"),
            };
            (i, what)
        })
        .or_else(|| {
            (run.standing != recorded.standing).then(|| {
                let how = match &run.standing {
                    Standing::Done(answer) => format!(
                        "it ended with another answer: {}",
                        answer.text.lines().next().unwrap_or_default()
                    ),
                    Standing::Stopped(why) => format!("it stopped: {why}"),
                    Standing::Going => "it did not end".to_string(),
                };
                (run.ran.len(), how)
            })
        });
    Ok(Replayed { run, differs })
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
    if let Some((k, of)) = activation.attempt {
        prompt.push_str(&format!(
            "\n\nThis is attempt {k} of {of}, made apart from the others: work it out on your own."
        ));
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
pub const TEMPLATES: [&str; 5] = [
    "fan-out",
    "work-check",
    "draft-review",
    "self-consistency",
    "orchestrator",
];

/// A node as a template has it: one copy, any tools, a join waiting for
/// all.
fn plain(id: &str, kind: Kind, instructions: &str, output: Option<Value>) -> Node {
    Node {
        id: id.into(),
        kind,
        instructions: instructions.into(),
        tools: None,
        model: None,
        output,
        max_visits: default_max_visits(),
        copies: 1,
        read_only: false,
        wait: Wait::All,
    }
}

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
    let node = plain;
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
        "self-consistency" => Graph {
            name: name.into(),
            start: "solve".into(),
            end: Some("vote".into()),
            nodes: vec![
                Node {
                    copies: 3,
                    ..node(
                        "solve",
                        Kind::Agent,
                        "Do the task, and answer with the answer alone: a figure, a name, a \
                         yes or no, as the task asks.",
                        None,
                    )
                },
                node("vote", Kind::Vote, "The answer given most often.", None),
            ],
            edges: vec![edge("solve", "vote", When::Always)],
            max_runs: default_max_runs(),
        },
        "orchestrator" => Graph {
            name: name.into(),
            start: "plan".into(),
            end: Some("write".into()),
            nodes: vec![
                Node {
                    max_visits: 2,
                    ..node(
                        "plan",
                        Kind::Agent,
                        "Break the task into questions to look up, two to six, each said in \
                         full, and look up none of them yourself. If a verification came back, \
                         list under known the findings it found sound, and ask only what it \
                         found missing.",
                        Some(json!({
                            "type": "object",
                            "properties": {
                                "questions": {"type": "array", "items": {"type": "string"}},
                                "known": {"type": "array", "items": {"type": "string"}}
                            },
                            "required": ["questions"]
                        })),
                    )
                },
                Node {
                    max_visits: MAX_FAN_OUT as u32,
                    read_only: true,
                    ..node(
                        "research",
                        Kind::Agent,
                        "Look up your question with the tools you have, which only read (files, \
                         the ledger, memory), and report what you found and where you found it.",
                        None,
                    )
                },
                Node {
                    max_visits: 2,
                    ..node(
                        "verify",
                        Kind::Join,
                        "Check the findings that reached you, and those the plan lists as known, \
                         against each other and against the task, as someone who looked up none \
                         of them. Pass them only if together they answer the task and agree, and \
                         then restate the findings that answer it; otherwise say what is wrong or \
                         missing, and which findings hold.",
                        Some(check.clone()),
                    )
                },
                node(
                    "write",
                    Kind::Agent,
                    "Write the answer to the task from the findings the verification passed.",
                    None,
                ),
            ],
            edges: vec![
                edge(
                    "plan",
                    "research",
                    When::Each {
                        field: "questions".into(),
                    },
                ),
                edge("plan", "verify", When::Always),
                edge("research", "verify", When::Always),
                edge("verify", "write", When::Pass),
                edge("verify", "plan", When::Fail),
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
     writing that benefits from an editor; `self-consistency` for a question with one short \
     answer that one agent sometimes gets wrong (a figure, a name, a yes or no); `orchestrator` \
     for research that splits into questions, looked up with tools that only read, whose \
     findings one checker verifies before the answer is written. Answer with one JSON object: \
     {\"shape\": \"single\" | \"fan-out\" | \"work-check\" | \"draft-review\" | \
     \"self-consistency\" | \"orchestrator\", \"why\": \"…\"}.";

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
            "self-consistency" => {
                "Choose `self-consistency` for a question with one short answer that one agent \
                 sometimes gets wrong (a figure, a name, a yes or no)."
            }
            "orchestrator" => {
                "Choose `orchestrator` for research that splits into questions, looked up with \
                 tools that only read, whose findings one checker verifies before the answer is \
                 written."
            }
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
        let item = match ran.activation.attempt {
            Some((k, of)) => format!("{item} (attempt {k} of {of})"),
            None => item,
        };
        lines.push(match &ran.answer {
            Ok(answer) => {
                let first = answer.text.lines().next().unwrap_or_default();
                let short: String = first.chars().take(120).collect();
                let session = match (&answer.session, &answer.json) {
                    (Some(s), _) => format!(" (session {s})"),
                    // A vote's: how many gave its answer.
                    (None, Some(json)) if json.get("votes").is_some() => {
                        format!(" (given by {} of {})", json["votes"], json["of"])
                    }
                    _ => String::new(),
                };
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
            let to = match graph.node(&edge.to).map(|n| (n.kind, n.wait, n.copies)) {
                Some((Kind::Join, Wait::All, _)) => format!("{} (waits for all)", edge.to),
                Some((Kind::Join, Wait::First, _)) => format!("{} (takes the first)", edge.to),
                Some((Kind::Join, Wait::Quorum(n), _)) => format!("{} (waits for {n})", edge.to),
                Some((Kind::Vote, _, _)) => format!("{} (votes)", edge.to),
                Some((_, _, copies)) if copies > 1 => format!("{} (×{copies})", edge.to),
                _ => edge.to.clone(),
            };
            let from = match graph.node(&edge.from).map(|n| n.copies) {
                Some(copies) if copies > 1 => format!("{} (×{copies})", edge.from),
                _ => edge.from.clone(),
            };
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
    fn a_vote_takes_the_answer_its_copies_give_most_often() {
        let graph = template("self-consistency").unwrap();
        graph.check().unwrap();
        // The copies run together, so which of them gives which answer is
        // the threads' order: the vote does not depend on it.
        let script = Script::new(&[("solve", &["42", "41", "42"])]);
        let mut run = GraphRun::start(&graph, "What is six times seven?");
        let Standing::Done(answer) = run.run(&graph, &script, &mut |_| {}) else {
            panic!("{:?}", run.standing);
        };
        assert_eq!(answer.text, "42");
        assert_eq!(answer.json, Some(json!({"votes": 2, "of": 3})));
        assert_eq!(answer.tokens, 0, "a vote runs no session");
        let lines = summary(&run);
        assert_eq!(lines.last().unwrap(), "vote (given by 2 of 3): 42");
        assert!(lines[0].starts_with("solve (attempt "), "{lines:?}");
        let prompts = script.prompts.lock().unwrap();
        assert_eq!(prompts.len(), 3);
        for k in 1..=3 {
            let told = format!("This is attempt {k} of 3, made apart from the others");
            assert!(prompts.iter().any(|(_, p)| p.contains(&told)), "{told}");
        }
        assert_eq!(outline(&graph), "solve (×3) → vote (votes)");
        // Case, spacing and a closing full stop aside; a tie goes to the
        // earliest.
        let said = |texts: &[&str]| texts.iter().map(|t| t.to_string()).collect::<Vec<_>>();
        assert_eq!(
            vote(&said(&["Paris.", "  paris", "Rome"])),
            Some(("Paris.".to_string(), 2, 3))
        );
        assert_eq!(
            vote(&said(&["Rome", "Paris"])),
            Some(("Rome".to_string(), 1, 2))
        );
        assert_eq!(vote(&[]), None);
        let mut copied = graph.clone();
        copied.nodes[1].copies = 2;
        assert!(copied
            .check()
            .unwrap_err()
            .contains("a join runs in one copy"));
        copied.nodes[1].copies = 1;
        copied.nodes[1].output = Some(json!({"type": "object"}));
        assert!(copied.check().unwrap_err().contains("keeps to no schema"));
        copied.nodes[0].copies = 4;
        assert!(copied
            .check()
            .unwrap_err()
            .contains("copies is 1 to its max_visits"));
    }

    #[test]
    fn a_join_may_take_the_first_answer_or_a_quorum() {
        // Two ways to the join, one a node longer than the other.
        let edge = |from: &str, to: &str| Edge {
            from: from.into(),
            to: to.into(),
            when: When::Always,
        };
        let graph = |wait: Wait| Graph {
            name: "race".into(),
            start: "ask".into(),
            end: Some("join".into()),
            nodes: vec![
                plain("ask", Kind::Agent, "Ask.", None),
                plain("quick", Kind::Agent, "Answer at once.", None),
                plain("slow", Kind::Agent, "Begin.", None),
                plain("slower", Kind::Agent, "Finish.", None),
                Node {
                    wait,
                    ..plain("join", Kind::Join, "Put them together.", None)
                },
            ],
            edges: vec![
                edge("ask", "quick"),
                edge("ask", "slow"),
                edge("slow", "slower"),
                edge("quick", "join"),
                edge("slower", "join"),
            ],
            max_runs: default_max_runs(),
        };
        let joined = |wait: Wait| {
            let graph = graph(wait);
            graph.check().unwrap();
            let script = Script::new(&[
                ("ask", &["go"]),
                ("quick", &["Q"]),
                ("slow", &["S"]),
                ("slower", &["SS"]),
                ("join", &["joined"]),
            ]);
            let mut run = GraphRun::start(&graph, "Race.");
            assert!(matches!(
                run.run(&graph, &script, &mut |_| {}),
                Standing::Done(_)
            ));
            assert_eq!(run.visits("join"), 1);
            let join = run
                .ran
                .iter()
                .find(|r| r.activation.node == "join")
                .unwrap();
            let inputs: Vec<String> = join
                .activation
                .inputs
                .iter()
                .map(|(n, _)| n.clone())
                .collect();
            let order: Vec<String> = run.ran.iter().map(|r| r.activation.node.clone()).collect();
            (order, inputs, run.closed.clone())
        };
        let (order, inputs, closed) = joined(Wait::First);
        assert_eq!(order, ["ask", "quick", "slow", "join", "slower"]);
        assert_eq!(inputs, ["quick"], "what came later was not passed on");
        assert!(closed.contains("join"));
        let (order, inputs, closed) = joined(Wait::All);
        assert_eq!(order, ["ask", "quick", "slow", "slower", "join"]);
        assert_eq!(inputs, ["quick", "slower"]);
        assert!(closed.is_empty());
        let (_, inputs, _) = joined(Wait::Quorum(2));
        assert_eq!(inputs, ["quick", "slower"]);
        let mut wrong = graph(Wait::Quorum(0));
        assert!(wrong.check().unwrap_err().contains("at least 1"));
        wrong.nodes[4].wait = Wait::All;
        wrong.nodes[0].wait = Wait::First;
        assert!(wrong.check().unwrap_err().contains("only a join waits"));
    }

    #[test]
    fn an_orchestrator_has_its_findings_verified_before_it_writes() {
        let graph = template("orchestrator").unwrap();
        assert!(graph.node("research").unwrap().read_only);
        let script = Script::new(&[
            (
                "plan",
                &[
                    r#"{"questions": ["the price", "the hosting"], "known": []}"#,
                    r#"{"questions": ["the support"], "known": ["12 USD a month", "Frankfurt"]}"#,
                ],
            ),
            ("research", &["12 USD a month", "Frankfurt", "email only"]),
            (
                "verify",
                &[
                    r#"{"pass": false, "why": "nothing on support; the price and hosting hold"}"#,
                    r#"{"pass": true, "why": "12 USD a month, in Frankfurt, email support"}"#,
                ],
            ),
            (
                "write",
                &["12 USD a month, hosted in Frankfurt, with email support."],
            ),
        ]);
        let mut run = GraphRun::start(&graph, "Tell me about the plan.");
        let Standing::Done(answer) = run.run(&graph, &script, &mut |_| {}) else {
            panic!("{:?}", run.standing);
        };
        assert_eq!(
            answer.text,
            "12 USD a month, hosted in Frankfurt, with email support."
        );
        assert_eq!(
            (
                run.visits("plan"),
                run.visits("research"),
                run.visits("verify")
            ),
            (2, 3, 2)
        );
        // The second verification is told the plan's known findings with
        // the new one; the plan was told what the first found missing.
        let prompts = script.prompts.lock().unwrap();
        let of = |node: &str| -> Vec<&String> {
            prompts
                .iter()
                .filter(|(n, _)| n == node)
                .map(|(_, p)| p)
                .collect()
        };
        assert!(of("plan")[1].contains("nothing on support"));
        let second = of("verify")[1];
        assert!(
            second.contains("From plan:")
                && second.contains("Frankfurt")
                && second.contains("email only"),
            "{second}"
        );
        assert_eq!(
            outline(&graph),
            "plan → research for each of its questions; plan → verify (waits for all); \
             research → verify (waits for all); verify → write if it passes; verify → plan if it \
             fails"
        );
    }

    #[test]
    fn a_run_replays_as_it_ran_until_the_graph_changes() {
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
        run.run(&graph, &script, &mut |_| {});
        let same = replay(&graph, &run).unwrap();
        assert_eq!(same.differs, None, "{same:?}");
        assert_eq!(same.run, run);
        // A graph whose check no longer loops back: the replay goes another
        // way where the change takes effect.
        let mut changed = graph.clone();
        changed.edges.retain(|e| e.when != When::Fail);
        assert_ne!(digest(&changed), digest(&graph));
        let other = replay(&changed, &run).unwrap();
        let (at, why) = other.differs.expect("the changed graph routes otherwise");
        assert_eq!(at, 2, "{why}");
    }

    #[test]
    fn a_run_goes_on_after_a_node_that_waited() {
        struct Waits;
        impl Executor for Waits {
            fn run(&self, node: &Node, _: &str, _: bool) -> Result<Answer, String> {
                match node.id.as_str() {
                    "split" => Ok(Answer {
                        text: r#"{"parts": ["a", "b"]}"#.into(),
                        json: None,
                        session: None,
                        tokens: 1,
                        outside: false,
                    }),
                    "part" => Err("session s1 waits for a person's approval of ledger_pay".into()),
                    _ => Ok(Answer {
                        text: "both".into(),
                        json: None,
                        session: None,
                        tokens: 1,
                        outside: false,
                    }),
                }
            }
        }
        let graph = template("fan-out").unwrap();
        let mut run = GraphRun::start(&graph, "Two things.");
        let Standing::Stopped(why) = run.run(&graph, &Waits, &mut |_| {}) else {
            panic!("{:?}", run.standing);
        };
        assert!(why.starts_with("part failed: session s1 waits"), "{why}");
        // Both parts of the wave were kept, though the first failed.
        assert_eq!(run.visits("part"), 2);
        let answer = |text: &str| Answer {
            text: text.into(),
            json: None,
            session: None,
            tokens: 1,
            outside: false,
        };
        run.resume(&graph, 1, answer("A")).unwrap();
        assert!(
            matches!(run.standing, Standing::Stopped(_)),
            "one part still failed"
        );
        run.resume(&graph, 2, answer("B")).unwrap();
        assert_eq!(run.standing, Standing::Going);
        assert_eq!(
            run.run(&graph, &Waits, &mut |_| {}),
            Standing::Done(answer("both"))
        );
        assert!(run.resume(&graph, 0, answer("x")).is_err());
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
