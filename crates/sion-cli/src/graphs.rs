//! Graphs of agents from the command line (`sion agent graph`): a profile's
//! agent run as the nodes of a graph ([`sion_agent::graph`]), each node a
//! session of its own, the run kept under `$SION_HOME/agents/graphs`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
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
                serde_json::from_str(&text)
                    .map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))
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
    fn run(&self, node: &Node, prompt: &str, outside: bool) -> Result<Answer, String> {
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
                // A node is one step of the graph: it does not stop to write
                // down how it did it.
                reflect: false,
                ..profile.equipment()
            },
            crate::runtime::Hooks {
                outside,
                ..crate::runtime::Hooks::default()
            },
        )
        .map_err(|e| e.to_string())?;
        let mut used = saved.state.usage;
        used.add(&saved.state.delegated);
        match &saved.ended {
            Some(Ended::Done { text }) => Ok(Answer {
                text: text.clone(),
                json: None,
                session: Some(saved.state.id.clone()),
                tokens: used.total(),
                // What it says may be steered by what it read from outside.
                outside: saved.state.standing.tainted_until.is_some(),
            }),
            Some(Ended::NeedsApproval { tool, .. }) => Err(format!(
                "session {} waits for a person's approval of {tool}",
                saved.state.id
            )),
            _ => Err(format!(
                "session {} ended without an answer: {}",
                saved.state.id,
                crate::runtime::last_words(&saved)
            )),
        }
    }
}

/// Asks the profile's first model how to work on `task`: one agent, or a
/// graph from the templates ([`sion_agent::graph::ROUTER`]).
pub fn route(profile: &Profile, task: &str) -> Result<Route, CliError> {
    let spec = profile
        .models
        .first()
        .ok_or_else(|| CliError::Usage(format!("profile {} has no model", profile.name)))?;
    let model = crate::runtime::model(spec, 0)?;
    let request = sion_agent::model::Request {
        system: sion_agent::graph::ROUTER.into(),
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
    Ok(sion_agent::graph::route_of(&text))
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
    let mut run = kept.run.clone();
    let mut failed = None;
    run.run(&graph, &executor, &mut |run| {
        let copy = Kept {
            id: id.clone(),
            profile: name.clone(),
            graph: graph.clone(),
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
        /// A template: fan-out, work-check or draft-review.
        #[arg(long, conflicts_with_all = ["graph", "route"])]
        template: Option<String>,
        /// A graph of your own: a JSON file.
        #[arg(long, conflicts_with = "route")]
        graph: Option<PathBuf>,
        /// Ask the profile's model first how to work on the task: one agent
        /// (a single session), or one of the templates.
        #[arg(long)]
        route: bool,
    },
    /// Show a graph's run: what each node answered, and how it stands.
    Show {
        /// The run, or the start of its id.
        run: String,
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
                println!("{name:<14}{}", outline(&named(name)?));
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
                    let chosen = route(&profile, &task)?;
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
    }
    Ok(())
}
