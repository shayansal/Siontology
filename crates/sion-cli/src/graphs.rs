//! Graphs of agents from the command line (`sion agent graph`): a profile's
//! agent run as the nodes of a graph ([`sion_agent::graph`]), each node a
//! session of its own, the run kept under `$SION_HOME/agents/graphs`.

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sion_agent::compare::{Arm, Comparison, TaskSet};
use sion_agent::graph::{
    outline, template, Answer, Executor, Graph, GraphRun, Node, Route, Standing, TEMPLATES,
};
use sion_agent::Ended;

use crate::error::CliError;
use crate::profiles::Profile;
use crate::runtime::SharedLedger;
use crate::session::wallet_dir;

/// A graph's run as it is kept: the graph, the profile, and the run.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Kept {
    /// Its id.
    pub id: String,
    /// The profile whose agent runs the nodes.
    pub profile: String,
    /// The graph.
    pub graph: Graph,
    /// The digest of the graph the run began with
    /// ([`sion_agent::graph::digest`]): a kept graph that no longer has it
    /// was changed after the run began, and is refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned: Option<sion_core::hash::Hash32>,
    /// The run.
    pub run: GraphRun,
}

/// Where graphs' runs are kept: `$SION_HOME/agents/graphs`.
pub fn dir() -> PathBuf {
    wallet_dir()
        .parent()
        .map(|home| home.join("agents").join("graphs"))
        .unwrap_or_else(|| PathBuf::from(".sion/agents/graphs"))
}

impl Kept {
    /// Writes it (to a sibling first, then renamed).
    pub fn save(&self) -> Result<(), CliError> {
        let dir = dir();
        std::fs::create_dir_all(&dir).map_err(|e| CliError::Usage(e.to_string()))?;
        let path = dir.join(format!("{}.json", self.id));
        let text =
            serde_json::to_string_pretty(self).map_err(|e| CliError::Usage(e.to_string()))?;
        let partial = path.with_extension("partial");
        std::fs::write(&partial, text)
            .and_then(|()| std::fs::rename(&partial, &path))
            .map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))
    }

    /// Loads the run `id` (or a unique prefix of it).
    pub fn load(id: &str) -> Result<Kept, CliError> {
        let dir = dir();
        let found: Vec<PathBuf> = std::fs::read_dir(&dir)
            .map_err(|e| CliError::Usage(format!("{}: {e}", dir.display())))?
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.extension().is_some_and(|x| x == "json")
                    && p.file_stem()
                        .and_then(|s| s.to_str())
                        .is_some_and(|s| s.starts_with(id))
            })
            .collect();
        match found.as_slice() {
            [path] => {
                let text = std::fs::read_to_string(path)
                    .map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?;
                let kept: Kept = serde_json::from_str(&text)
                    .map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?;
                let now = sion_agent::graph::digest(&kept.graph);
                match kept.pinned {
                    Some(pinned) if pinned != now => Err(CliError::Usage(format!(
                        "graph run {}: its graph was changed after the run began (it began \
                         with graph {}, the run's file now holds {})",
                        kept.id,
                        pinned.short(),
                        now.short()
                    ))),
                    _ => Ok(kept),
                }
            }
            [] => Err(CliError::Usage(format!("no graph run {id}"))),
            _ => Err(CliError::Usage(format!(
                "{id} names more than one graph run"
            ))),
        }
    }
}

/// Runs each node as a session of the profile's agent: the node's model
/// first if it names one, only its tools if it names them, tainted from
/// the start if what reached it came from a session that read content from
/// outside.
pub struct Sessions<'a> {
    /// The ledger.
    pub ledger: SharedLedger,
    /// The profile.
    pub profile: &'a Profile,
}

impl std::fmt::Debug for Sessions<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sessions")
            .field("profile", &self.profile.name)
            .finish()
    }
}

impl Executor for Sessions<'_> {
    fn run(&self, node: &Node, prompt: &str, held: sion_agent::Held) -> Result<Answer, String> {
        let profile = self.profile;
        let mut models = profile.models.clone();
        if let Some(model) = &node.model {
            models.retain(|m| m != model);
            models.insert(0, model.clone());
        }
        let (saved, _) = crate::runtime::start_watched(
            self.ledger.clone(),
            &profile.wallet,
            profile.agent,
            prompt,
            &[],
            models,
            profile.policy(),
            crate::runtime::Equipment {
                only_tools: node.tools.clone(),
                read_only: node.read_only,
                // A node is one step of the graph: it does not stop to write
                // down how it did it.
                reflect: false,
                ..profile.equipment()
            },
            crate::runtime::Hooks {
                outside: held.tainted,
                confidential: held.confidential,
                ..crate::runtime::Hooks::default()
            },
        )
        .map_err(|e| e.to_string())?;
        answer_of(&saved)
    }
}

/// A node's answer from its session as it ended: what it said, or why it
/// has none (it waits for a person's approval, or ended without one).
fn answer_of(saved: &crate::runtime::Saved) -> Result<Answer, String> {
    let mut used = saved.state.usage;
    used.add(&saved.state.delegated);
    match &saved.ended {
        Some(Ended::Done { text }) => Ok(Answer {
            text: text.clone(),
            json: None,
            session: Some(saved.state.id.clone()),
            tokens: used.total(),
            // What it says may be steered by what it read from outside, and
            // may carry what it held confidential.
            outside: saved.state.standing.tainted_until.is_some(),
            confidential: saved.state.standing.confidential,
        }),
        Some(Ended::NeedsApproval { tool, .. }) => Err(format!(
            "session {} waits for a person's approval of {tool}",
            saved.state.id
        )),
        _ => Err(format!(
            "session {} ended without an answer: {}",
            saved.state.id,
            crate::runtime::last_words(saved)
        )),
    }
}

/// Asks the profile's first model how to work on `task`: one agent, or a
/// graph from the templates ([`sion_agent::graph::ROUTER`]).
pub fn route(profile: &Profile, task: &str) -> Result<Route, CliError> {
    route_among(profile, task, &TEMPLATES)
}

/// How to work on `task` for `profile`, among one agent and the templates
/// that earned its traffic in a comparison ([`earned`]): one agent, without
/// asking, if none has.
pub fn route_earned(profile: &Profile, task: &str) -> Result<Route, CliError> {
    let earned = earned(&profile.name)?;
    let shapes: Vec<&str> = TEMPLATES
        .iter()
        .copied()
        .filter(|t| earned.contains(*t))
        .collect();
    if shapes.is_empty() {
        return Ok(Route {
            shape: "single".into(),
            why: format!(
                "no template has beaten one agent in profile {}'s comparisons \
                 (sion agent graph eval)",
                profile.name
            ),
        });
    }
    route_among(profile, task, &shapes)
}

/// Asks the profile's first model how to work on `task`, among one agent
/// and `shapes`.
pub fn route_among(profile: &Profile, task: &str, shapes: &[&str]) -> Result<Route, CliError> {
    let spec = profile
        .models
        .first()
        .ok_or_else(|| CliError::Usage(format!("profile {} has no model", profile.name)))?;
    let model = crate::runtime::model(spec, 0)?;
    let request = sion_agent::model::Request {
        system: sion_agent::graph::router_for(shapes),
        messages: vec![sion_agent::Message::user(format!("The task:\n{task}"))],
        tools: Vec::new(),
        max_tokens: 512,
        images: Default::default(),
    };
    let response = model
        .complete(&request)
        .map_err(|e| CliError::Usage(format!("the router could not answer: {e}")))?;
    let text = sion_agent::Message {
        role: sion_agent::Role::Assistant,
        content: response.content,
    }
    .text();
    Ok(sion_agent::graph::route_among_of(&text, shapes))
}

/// Where a profile's comparisons are kept: `$SION_HOME/agents/evals/<profile>`,
/// the latest for each set.
pub fn evals_dir(profile: &str) -> PathBuf {
    dir()
        .parent()
        .map(|agents| agents.join("evals").join(profile))
        .unwrap_or_else(|| PathBuf::from(".sion/agents/evals").join(profile))
}

/// Keeps `comparison` for `profile`, in place of the one before for the
/// same set.
pub fn keep_comparison(profile: &str, comparison: &Comparison) -> Result<PathBuf, CliError> {
    let dir = evals_dir(profile);
    std::fs::create_dir_all(&dir).map_err(|e| CliError::Usage(e.to_string()))?;
    let path = dir.join(format!("{}.json", comparison.digest.to_hex()));
    let text =
        serde_json::to_string_pretty(comparison).map_err(|e| CliError::Usage(e.to_string()))?;
    let partial = path.with_extension("partial");
    std::fs::write(&partial, text)
        .and_then(|()| std::fs::rename(&partial, &path))
        .map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?;
    Ok(path)
}

/// The graphs that earned `profile`'s traffic: those that beat both
/// baselines in its latest comparison on some set.
pub fn earned(profile: &str) -> Result<BTreeSet<String>, CliError> {
    let dir = evals_dir(profile);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(BTreeSet::new());
    };
    let mut earned = BTreeSet::new();
    for path in entries.flatten().map(|e| e.path()) {
        if path.extension().is_some_and(|x| x == "json") {
            let text = std::fs::read_to_string(&path)
                .map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?;
            let comparison: Comparison = serde_json::from_str(&text)
                .map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?;
            earned.extend(comparison.earned());
        }
    }
    Ok(earned)
}

/// Compares, with `profile`'s models, one agent, one agent `samples` times
/// with its answers put to a vote, and each of `graphs`, on the set of
/// tasks in `set` (its exact bytes): each case run `runs` times by each,
/// with `budget_tokens` for a case. With `judge` (a model's spec), a judge
/// reads each run's steps.
pub fn evaluate(
    profile: &Profile,
    set: &[u8],
    graphs: Vec<Graph>,
    samples: u32,
    runs: u32,
    budget_tokens: u64,
    judge: Option<&str>,
) -> Result<Comparison, CliError> {
    let tasks = TaskSet::parse(set).map_err(CliError::Usage)?;
    if graphs.is_empty() {
        return Err(CliError::Usage(
            "give a graph to compare with one agent: --template or --graph".into(),
        ));
    }
    let mut arms = vec![Arm::Single, Arm::Vote { samples }];
    arms.extend(graphs.into_iter().map(|graph| Arm::Graph { graph }));
    let specs = profile.models.clone();
    if specs.is_empty() {
        return Err(CliError::Usage(format!(
            "profile {} has no model",
            profile.name
        )));
    }
    let models = move || -> Result<Vec<Box<dyn sion_agent::model::Model>>, String> {
        specs
            .iter()
            .map(|spec| crate::runtime::model(spec, 0).map_err(|e| e.to_string()))
            .collect()
    };
    let judge = judge
        .map(|spec| crate::runtime::model(spec, 0))
        .transpose()?;
    sion_agent::compare::compare(
        &tasks,
        sion_agent::evals::digest(set),
        &arms,
        &models,
        judge.as_deref(),
        budget_tokens,
        runs,
        &sion_agent::Limits::default(),
    )
    .map_err(CliError::Usage)
}

/// Runs `graph` on `task` with `profile`'s agent, keeping the run after
/// each wave; returns it and the lines to show.
pub fn run(
    ledger: SharedLedger,
    profile: &Profile,
    graph: Graph,
    task: &str,
) -> Result<(Kept, Vec<String>), CliError> {
    graph.check().map_err(CliError::Usage)?;
    let kept = Kept {
        id: crate::runtime::new_id(),
        profile: profile.name.clone(),
        pinned: Some(sion_agent::graph::digest(&graph)),
        run: GraphRun::start(&graph, task),
        graph,
    };
    kept.save()?;
    go_on(ledger, profile, kept)
}

/// Goes on with a kept run until it is done or stops.
pub fn go_on(
    ledger: SharedLedger,
    profile: &Profile,
    mut kept: Kept,
) -> Result<(Kept, Vec<String>), CliError> {
    let executor = Sessions { ledger, profile };
    let graph = kept.graph.clone();
    let id = kept.id.clone();
    let name = kept.profile.clone();
    let pinned = kept.pinned;
    let mut run = kept.run.clone();
    let mut failed = None;
    run.run(&graph, &executor, &mut |run| {
        let copy = Kept {
            id: id.clone(),
            profile: name.clone(),
            graph: graph.clone(),
            pinned,
            run: run.clone(),
        };
        if let Err(e) = copy.save() {
            failed = Some(e);
        }
    });
    if let Some(e) = failed {
        return Err(e);
    }
    kept.run = run;
    let lines = shown(&kept);
    Ok((kept, lines))
}

/// Goes on with a kept run that stopped at nodes whose sessions waited for
/// a person: each such session, approved and run on since
/// (`sion agent continue`), gives its node's answer, and the run goes on.
pub fn go_on_after_approval(
    ledger: SharedLedger,
    profile: &Profile,
    mut kept: Kept,
) -> Result<(Kept, Vec<String>), CliError> {
    let waiting: Vec<(usize, String)> = kept
        .run
        .ran
        .iter()
        .enumerate()
        .filter_map(|(i, ran)| {
            let why = ran.answer.as_ref().err()?;
            let session = why.strip_prefix("session ")?.split(' ').next()?;
            why.contains(" waits for a person's approval")
                .then(|| (i, session.to_string()))
        })
        .collect();
    if waiting.is_empty() {
        return Err(CliError::Usage(format!(
            "graph run {} did not stop at a node that waits for a person",
            kept.id
        )));
    }
    let mut still = Vec::new();
    for (index, session) in waiting {
        let saved = crate::runtime::Saved::load(&session)?;
        match answer_of(&saved) {
            Ok(answer) => kept
                .run
                .resume(&kept.graph, index, answer)
                .map_err(CliError::Usage)?,
            Err(why) if why.contains(" waits for a person's approval") => still.push(format!(
                "{why}: approve its call (sion agent continue {session} --approve) or decline \
                 it (--decline <why>), then continue this run again"
            )),
            Err(why) => still.push(format!("{why}: its node stays failed")),
        }
    }
    kept.save()?;
    let (kept, mut lines) = if kept.run.standing == Standing::Going {
        go_on(ledger, profile, kept)?
    } else {
        let lines = shown(&kept);
        (kept, lines)
    };
    lines.extend(still);
    Ok((kept, lines))
}

/// What is shown of a kept run: the graph, each node as it ran, and how
/// the run stands.
pub fn shown(kept: &Kept) -> Vec<String> {
    let mut lines = vec![format!(
        "graph {} ({}) on profile {}",
        kept.id, kept.graph.name, kept.profile
    )];
    lines.extend(
        sion_agent::graph::summary(&kept.run)
            .into_iter()
            .map(|l| format!("  {l}")),
    );
    lines.push(match &kept.run.standing {
        Standing::Done(answer) => format!("answer: {}", answer.text.trim()),
        Standing::Stopped(why) => format!("stopped: {why}"),
        Standing::Going => "still going".into(),
    });
    lines
}

/// Graphs of agents: a profile's agent run as the nodes of a small graph.
#[derive(Debug, clap::Subcommand)]
pub enum GraphCommand {
    /// The graphs the runtime knows, a line each; with a name, that graph
    /// as JSON, to start a graph of your own from.
    Templates {
        /// A template's name.
        name: Option<String>,
    },
    /// Run a graph on a task, each of its nodes a session of a profile's
    /// agent: a template, a graph of your own, or what the profile's model
    /// says the task needs.
    Run {
        /// The profile whose agent runs the nodes.
        profile: String,
        /// The task.
        #[arg(long)]
        task: String,
        /// A template: fan-out, work-check, draft-review, self-consistency
        /// or orchestrator.
        #[arg(long, conflicts_with_all = ["graph", "route"])]
        template: Option<String>,
        /// A graph of your own: a JSON file.
        #[arg(long, conflicts_with = "route")]
        graph: Option<PathBuf>,
        /// Ask the profile's model first how to work on the task: one agent
        /// (a single session), or one of the templates that beat one agent
        /// in the profile's comparisons (`sion agent graph eval`).
        #[arg(long)]
        route: bool,
        /// With --route, let the router choose any template, measured or
        /// not.
        #[arg(long, requires = "route")]
        untested: bool,
    },
    /// Compare ways of working through a set of tasks, with a profile's
    /// models: one agent, one agent several times with its answers put to
    /// a vote, and each graph given, on the same token budget for a case,
    /// each case run several times, with the cases' stand-in tools and
    /// checks. A graph that beats both (by Newcombe's 95% interval for the
    /// difference of pass rates) earns the profile's traffic: `run --route`
    /// offers only templates that have.
    Eval {
        /// The profile whose models do the work.
        profile: String,
        /// The set of tasks: a JSON file of cases, as an evaluation set's.
        #[arg(long, value_name = "FILE")]
        set: PathBuf,
        /// A template to compare (repeat for more).
        #[arg(long = "template", value_name = "NAME")]
        templates: Vec<String>,
        /// A graph of your own to compare: a JSON file (repeat for more).
        #[arg(long = "graph", value_name = "FILE")]
        graphs: Vec<PathBuf>,
        /// How many times one agent works on a case for the vote.
        #[arg(long, default_value_t = 3)]
        samples: u32,
        /// How many times each case is run by each way of working.
        #[arg(long, default_value_t = 5)]
        runs: u32,
        /// The tokens each way of working may use on a case, its sessions
        /// together.
        #[arg(long, default_value_t = 30_000)]
        budget_tokens: u64,
        /// A model that judges each run step by step, its agreement with
        /// labelled steps it was not shown measured first.
        #[arg(long, value_name = "MODEL")]
        judge: Option<String>,
    },
    /// Show a graph's run: what each node answered, and how it stands.
    Show {
        /// The run, or the start of its id.
        run: String,
    },
    /// Go on with a graph's run that stopped at nodes whose sessions waited
    /// for a person, once those sessions were approved and run on
    /// (`sion agent continue <session> --approve`).
    Continue {
        /// The run, or the start of its id.
        run: String,
    },
    /// Replay a graph's run from what it recorded: each node answered as it
    /// was, the graph (as it is kept with the run) must ask the same and
    /// route the answers the same way, to the same end.
    Replay {
        /// The run, or the start of its id.
        run: String,
        /// Replay it through this graph instead (a JSON file): to see
        /// whether a changed graph would have routed the run the same way.
        #[arg(long, value_name = "FILE")]
        graph: Option<PathBuf>,
    },
}

/// Runs a graph command.
pub fn command(ledger: SharedLedger, command: GraphCommand) -> Result<(), CliError> {
    let print = |lines: Vec<String>| lines.iter().for_each(|line| println!("{line}"));
    let named = |name: &str| {
        template(name).ok_or_else(|| {
            CliError::Usage(format!(
                "no template {name}; the templates: {}",
                TEMPLATES.join(", ")
            ))
        })
    };
    match command {
        GraphCommand::Templates { name: None } => {
            for name in TEMPLATES {
                println!("{name:<18}{}", outline(&named(name)?));
            }
        }
        GraphCommand::Templates { name: Some(name) } => {
            let graph = named(&name)?;
            let text =
                serde_json::to_string_pretty(&graph).map_err(|e| CliError::Usage(e.to_string()))?;
            println!("{text}");
        }
        GraphCommand::Run {
            profile,
            task,
            template: chosen,
            graph,
            route: routed,
            untested,
        } => {
            let profile = crate::profiles::get(&profile)?;
            let graph = match (chosen, graph, routed) {
                (Some(name), _, _) => named(&name)?,
                (None, Some(path), _) => {
                    let text = std::fs::read_to_string(&path)
                        .map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?;
                    serde_json::from_str(&text)
                        .map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?
                }
                (None, None, true) => {
                    let chosen = if untested {
                        route(&profile, &task)?
                    } else {
                        route_earned(&profile, &task)?
                    };
                    println!("route: {} ({})", chosen.shape, chosen.why);
                    match template(&chosen.shape) {
                        Some(graph) => graph,
                        None => {
                            let (saved, lines) = profile.start(ledger, &task)?;
                            println!(
                                "session {} for agent {}",
                                saved.state.id,
                                profile.agent.short()
                            );
                            print(lines);
                            return Ok(());
                        }
                    }
                }
                (None, None, false) => {
                    return Err(CliError::Usage(
                        "give a graph: --template, --graph or --route".into(),
                    ))
                }
            };
            let (_, lines) = run(ledger, &profile, graph, &task)?;
            print(lines);
        }
        GraphCommand::Show { run } => print(shown(&Kept::load(&run)?)),
        GraphCommand::Continue { run } => {
            let kept = Kept::load(&run)?;
            let profile = crate::profiles::get(&kept.profile)?;
            let (_, lines) = go_on_after_approval(ledger, &profile, kept)?;
            print(lines);
        }
        GraphCommand::Replay { run, graph } => {
            let kept = Kept::load(&run)?;
            let graph = match graph {
                Some(path) => {
                    let text = std::fs::read_to_string(&path)
                        .map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?;
                    serde_json::from_str(&text)
                        .map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?
                }
                None => kept.graph.clone(),
            };
            let replayed = sion_agent::graph::replay(&graph, &kept.run).map_err(CliError::Usage)?;
            match replayed.differs {
                None => println!(
                    "graph run {} replays as it ran: {} node runs, routed the same way, to the \
                     same end (graph {})",
                    kept.id,
                    kept.run.ran.len(),
                    sion_agent::graph::digest(&graph).short()
                ),
                Some((at, why)) => {
                    return Err(CliError::Verification(format!(
                        "graph run {} replays otherwise at node run {}: {why}",
                        kept.id,
                        at + 1
                    )))
                }
            }
        }
        GraphCommand::Eval {
            profile,
            set,
            templates,
            graphs,
            samples,
            runs,
            budget_tokens,
            judge,
        } => {
            let profile = crate::profiles::get(&profile)?;
            let bytes = std::fs::read(&set)
                .map_err(|e| CliError::Usage(format!("{}: {e}", set.display())))?;
            let mut compared = Vec::new();
            for name in &templates {
                compared.push(named(name)?);
            }
            for path in &graphs {
                let text = std::fs::read_to_string(path)
                    .map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?;
                compared.push(
                    serde_json::from_str(&text)
                        .map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?,
                );
            }
            let comparison = evaluate(
                &profile,
                &bytes,
                compared,
                samples,
                runs,
                budget_tokens,
                judge.as_deref(),
            )?;
            print(comparison.lines());
            let path = keep_comparison(&profile.name, &comparison)?;
            println!("kept: {}", path.display());
        }
    }
    Ok(())
}
