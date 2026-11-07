//! Agent profiles: how to run an agent, kept under a name, so that
//! schedules and channels can start and continue its sessions.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sion_agent::{Autonomy, Policy};
use sion_core::ids::{AtomId, ObjectId};

use crate::error::CliError;
use crate::runtime::{Equipment, Saved, SharedLedger};
use crate::session::wallet_dir;

/// How to run an agent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    /// Its name.
    pub name: String,
    /// The agent's wallet.
    pub wallet: String,
    /// The agent.
    pub agent: ObjectId,
    /// Its models, in order of preference.
    pub models: Vec<String>,
    /// How much it may do without asking.
    pub autonomy: Autonomy,
    /// Its memory log.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<ObjectId>,
    /// Its skills' directory (default: `$SION_HOME/agents/skills/<agent>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<PathBuf>,
    /// A directory it may read and write.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<PathBuf>,
    /// Whether it may fetch public web pages.
    #[serde(default)]
    pub web: bool,
    /// Whether it may hand work to other agents.
    #[serde(default)]
    pub delegate: bool,
    /// The models its subagents run with.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subagent_models: Vec<String>,
    /// The adjudicator of the tasks it posts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adjudicator: Option<AtomId>,
    /// Whether to ask it to write down long tasks as skills.
    #[serde(default = "yes")]
    pub reflect: bool,
    /// The sandbox its commands run in, if it has a shell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell: Option<sion_agent::sandbox::Sandbox>,
    /// The MCP servers whose reviewed tools it has, by name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp: Vec<String>,
    /// Whether it has the storage-deal tools.
    #[serde(default)]
    pub storage: bool,
    /// What its operator tells it beyond the fixed system prompt, which
    /// stays first (`sion agent evolve` improves it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    /// Whether a skill it reads may pre-approve the calls its
    /// `allowed-tools` names (never in a tainted session, never past a
    /// denial or its charter).
    #[serde(default = "yes")]
    pub skill_approvals: bool,
    /// Its browser and desktop.
    #[serde(default, skip_serializing_if = "crate::screens::Screen::is_empty")]
    pub screen: crate::screens::Screen,
    /// What makes pictures for it (`image_generate`, in its workspace):
    /// `openai:<model>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_model: Option<String>,
    /// Whether it has Home Assistant's tools ([`crate::home_assistant`]).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub home_assistant: bool,
    /// Tokens each run of its sessions may use, its subagents' included.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_tokens: Option<u64>,
    /// Micro-dollars each run of its sessions may spend, its subagents'
    /// included, at the models' prices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_micros: Option<u64>,
    /// The most turns a run of its sessions may take, if not the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<u32>,
    /// The most tokens each answer may have, if not the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// About how many tokens its sessions' conversations may reach before
    /// they are compacted, if not the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compact_at: Option<u64>,
    /// The model that writes a compacted conversation's summary
    /// ([`crate::runtime::Saved::summary_model`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_model: Option<String>,
    /// Whether its sessions go without a plan to keep.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_plan: bool,
    /// Whether its sessions may finish after changing code without running
    /// anything since.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_change_check: bool,
    /// Whether its sessions hold what tools return from outside in
    /// quarantine ([`sion_agent::quarantine`]).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub quarantine: bool,
    /// Curated logs its sessions keep in mind, read-only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shared_memory: Vec<ObjectId>,
    /// Whether its sessions hold confidential content from the start.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub confidential: bool,
    /// The paths of its workspace whose content is confidential.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub confidential_files: Vec<String>,
    /// Routes that choose its models for each task
    /// ([`crate::model_routes`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub routes: Vec<crate::model_routes::ModelRoute>,
    /// The model that sorts its tasks among its routes (default: its first
    /// model).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sorter: Option<String>,
}

fn yes() -> bool {
    true
}

impl Profile {
    /// The profile as it answers what others wrote, for anyone to read or
    /// for the operator later (X's mentions, a meeting's notes): its models
    /// and instructions, and none of its memory, skills (not even their
    /// names), workspace, browser, shell, MCP servers, web or helpers; its
    /// sessions are also to be given no tools
    /// ([`crate::channels::ChatSettings::tools`]).
    pub fn without_reach(&self) -> Profile {
        let mut public = self.clone();
        // A folder that holds no skill, and is never written to: none is
        // listed in the prompt.
        public.skills = Some(crate::secrets::dir().join("no-skills"));
        public.memory = None;
        public.shared_memory.clear();
        public.workspace = None;
        public.confidential_files.clear();
        public.web = false;
        public.delegate = false;
        public.shell = None;
        public.mcp.clear();
        public.storage = false;
        public.home_assistant = false;
        public.screen = Default::default();
        public.image_model = None;
        public.reflect = false;
        public
    }

    /// The rules its sessions run under.
    pub fn policy(&self) -> Policy {
        Policy {
            autonomy: self.autonomy,
            skill_approvals: self.skill_approvals,
            quarantine: self.quarantine,
            ..Policy::default()
        }
    }

    /// What its sessions may use besides the ledger.
    pub fn equipment(&self) -> Equipment {
        Equipment {
            workspace: self.workspace.clone(),
            web: self.web,
            memory: self.memory,
            skills: self.skills.clone(),
            system: None,
            reflect: self.reflect,
            delegate: self.delegate,
            subagent_models: self.subagent_models.clone(),
            adjudicator: self.adjudicator,
            depth: 0,
            shell: self.shell.clone(),
            mcp: self.mcp.clone(),
            storage: self.storage,
            // What its operator tells it, then what it learned from its
            // own work (`sion agent improve run`).
            instructions: crate::improve_loop::instructions_of(self),
            screen: self.screen.clone(),
            chat_instructions: None,
            only_tools: None,
            image_model: self.image_model.clone(),
            home_assistant: self.home_assistant,
            budget_tokens: self.budget_tokens,
            budget_micros: self.budget_micros,
            max_turns: self.max_turns,
            max_tokens: self.max_tokens,
            compact_at: self.compact_at,
            summary_model: self.summary_model.clone(),
            no_plan: self.no_plan,
            no_change_check: self.no_change_check,
            // A check is given to a run (`sion agent run --verify`), not to
            // a profile's sessions.
            verify: None,
            verify_files: None,
            shared_memory: self.shared_memory.clone(),
            read_only: false,
            reads_web: false,
            confidential: self.confidential,
            confidential_files: self.confidential_files.clone(),
            // A work item's session is started by the work runner
            // ([`Profile::start_work`]), a project's by its own
            // ([`Profile::start_project`]).
            work: None,
            project: None,
            session: None,
        }
    }

    /// Starts a session on project `link`: `task` is what it is asked to
    /// do for it. Its project tools note there, and its payments are held
    /// to what the project has left; it spawns no subagents, posts no tasks
    /// and makes no storage deals, which would move money past the budget.
    pub fn start_project(
        &self,
        ledger: SharedLedger,
        task: &str,
        link: crate::projects::ProjectLink,
        hooks: crate::runtime::Hooks<'_>,
    ) -> Result<(Saved, Vec<String>), CliError> {
        let choice = crate::model_routes::choose_for(
            self,
            &crate::model_routes::Task {
                text: task,
                pictures: 0,
                work: true,
            },
        );
        let started = crate::runtime::start_watched(
            ledger,
            &self.wallet,
            self.agent,
            task,
            &[],
            choice.models.clone(),
            self.policy(),
            // Its money moves only by payments held to the project's
            // budget: no subagents, tasks or storage deals of its own.
            Equipment {
                project: Some(link),
                delegate: false,
                storage: false,
                ..self.equipment()
            },
            hooks,
        );
        if let Ok((saved, _)) = &started {
            crate::model_routes::note(self, &choice, &choice.models, saved);
        }
        started
    }

    /// Starts session `session` (its id, fixed before it runs) on a work
    /// item ([`crate::work`]): `task` is its assignment, `link` the item it
    /// reports to.
    pub fn start_work(
        &self,
        ledger: SharedLedger,
        session: &str,
        task: &str,
        link: crate::work::WorkLink,
        hooks: crate::runtime::Hooks<'_>,
    ) -> Result<(Saved, Vec<String>), CliError> {
        let choice = crate::model_routes::choose_for(
            self,
            &crate::model_routes::Task {
                text: task,
                pictures: 0,
                work: true,
            },
        );
        let started = crate::runtime::start_watched(
            ledger,
            &self.wallet,
            self.agent,
            task,
            &[],
            choice.models.clone(),
            self.policy(),
            Equipment {
                work: Some(link),
                session: Some(session.to_string()),
                ..self.equipment()
            },
            hooks,
        );
        if let Ok((saved, _)) = &started {
            crate::model_routes::note(self, &choice, &choice.models, saved);
        }
        started
    }

    /// Starts a session on `task`.
    pub fn start(
        &self,
        ledger: SharedLedger,
        task: &str,
    ) -> Result<(Saved, Vec<String>), CliError> {
        self.start_watched(ledger, task, crate::runtime::Hooks::default())
    }

    /// [`Profile::start`], the run given `hooks`: watched, stopped and
    /// steered.
    pub fn start_watched(
        &self,
        ledger: SharedLedger,
        task: &str,
        hooks: crate::runtime::Hooks<'_>,
    ) -> Result<(Saved, Vec<String>), CliError> {
        self.start_in(
            ledger,
            task,
            &[],
            hooks,
            &crate::channels::ChatSettings::default(),
        )
    }

    /// [`Profile::start_watched`], as a chat's `settings` ask: its model
    /// first, its instructions after the operator's, only its tools.
    pub fn start_in(
        &self,
        ledger: SharedLedger,
        task: &str,
        pictures: &[sion_agent::Picture],
        hooks: crate::runtime::Hooks<'_>,
        settings: &crate::channels::ChatSettings,
    ) -> Result<(Saved, Vec<String>), CliError> {
        // The models its routes choose for the task, a chat's own choice
        // ahead of them.
        let choice = crate::model_routes::choose_for(
            self,
            &crate::model_routes::Task {
                text: task,
                pictures: pictures.len(),
                work: false,
            },
        );
        let mut models = choice.models.clone();
        if let Some(at) = settings
            .model
            .as_ref()
            .and_then(|chosen| models.iter().position(|spec| spec == chosen))
        {
            let chosen = models.remove(at);
            models.insert(0, chosen);
        }
        let started = crate::runtime::start_watched(
            ledger,
            &self.wallet,
            self.agent,
            task,
            pictures,
            models.clone(),
            self.policy(),
            crate::runtime::Equipment {
                chat_instructions: settings.instructions.clone(),
                only_tools: settings.tools.clone(),
                ..self.equipment()
            },
            hooks,
        );
        if let Ok((saved, _)) = &started {
            crate::model_routes::note(self, &choice, &models, saved);
        }
        started
    }
}

/// The file profiles are kept in: `$SION_HOME/agents/profiles.json`.
pub fn path() -> PathBuf {
    wallet_dir()
        .parent()
        .map(|home| home.join("agents").join("profiles.json"))
        .unwrap_or_else(|| PathBuf::from(".sion/agents/profiles.json"))
}

/// Every profile, by name.
pub fn all() -> Result<BTreeMap<String, Profile>, CliError> {
    let path = path();
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|e| CliError::Usage(format!("{}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(CliError::Usage(format!("{}: {e}", path.display()))),
    }
}

/// Writes every profile (to a sibling first, then renamed).
pub fn save_all(profiles: &BTreeMap<String, Profile>) -> Result<(), CliError> {
    write_json(&path(), profiles)
}

/// Writes `value` as JSON to `path`, through a sibling file renamed into
/// place.
pub fn write_json<T: Serialize>(path: &std::path::Path, value: &T) -> Result<(), CliError> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| CliError::Usage(format!("{}: {e}", dir.display())))?;
    }
    let text = serde_json::to_string_pretty(value).map_err(|e| CliError::Usage(e.to_string()))?;
    // A sibling of its own, so that two writers (two bridges, say) never
    // write into one file; the rename puts it in place whole.
    static WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let partial = path.with_extension(format!(
        "{}.{}.partial",
        std::process::id(),
        WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::write(&partial, text)
        .and_then(|()| std::fs::rename(&partial, path))
        .map_err(|e| {
            let _ = std::fs::remove_file(&partial);
            CliError::Usage(format!("{}: {e}", path.display()))
        })
}

/// The profile `name`.
pub fn get(name: &str) -> Result<Profile, CliError> {
    all()?.remove(name).ok_or_else(|| {
        CliError::Usage(format!(
            "no agent profile {name} (`sion agent profile add`)"
        ))
    })
}

/// Whether `name` is a profile name: 1 to 32 lowercase letters, digits and
/// hyphens.
pub fn valid_name(name: &str) -> bool {
    (1..=32).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// `sion agent profile …`
#[derive(Debug, clap::Subcommand)]
// Parsed once from the command line: the size of a variant does not matter.
#[allow(clippy::large_enum_variant)]
pub enum ProfileCommand {
    /// Add a profile, or replace the one of that name.
    Add {
        /// Its name (lowercase letters, digits and hyphens).
        name: String,
        /// The agent's wallet.
        wallet: String,
        /// The agent (its id).
        agent: String,
        /// The model, as for `sion agent run`. Repeat for fallbacks.
        #[arg(long = "model", required = true)]
        models: Vec<String>,
        /// How much it may do without asking, L0 to L6.
        #[arg(long, default_value = "L5")]
        autonomy: String,
        /// Its memory log (its id, or a unique prefix of one with a local
        /// store).
        #[arg(long)]
        memory: Option<String>,
        /// Its skills' directory.
        #[arg(long)]
        skills: Option<PathBuf>,
        /// A directory it may read and write.
        #[arg(long)]
        workspace: Option<PathBuf>,
        /// Let it fetch public web pages.
        #[arg(long)]
        web: bool,
        /// Let it hand work to other agents.
        #[arg(long)]
        delegate: bool,
        /// The model its subagents run with. Repeat for fallbacks.
        #[arg(long = "subagent-model")]
        subagent_models: Vec<String>,
        /// The adjudicator atom of the tasks it posts.
        #[arg(long)]
        adjudicator: Option<String>,
        /// Do not ask it to write down long tasks as skills.
        #[arg(long)]
        no_reflect: bool,
        #[command(flatten)]
        shell: crate::runtime::ShellArgs,
        /// Give it the reviewed tools of this MCP server. Repeat for more.
        #[arg(long = "mcp", value_name = "SERVER")]
        mcp: Vec<String>,
        /// Give it the storage-deal tools.
        #[arg(long)]
        storage: bool,
        #[command(flatten)]
        screen: crate::screens::ScreenArgs,
        /// What to tell it beyond the fixed system prompt.
        #[arg(long)]
        instructions: Option<String>,
        /// Ask before every call its autonomy does not cover, even those a
        /// skill it read pre-approves with `allowed-tools`.
        #[arg(long)]
        no_skill_approvals: bool,
        /// Let it make pictures in its workspace (`image_generate`):
        /// openai:<model>, such as openai:gpt-image-1 (OPENAI_API_KEY).
        #[arg(long, value_name = "PROVIDER:MODEL", requires = "workspace")]
        image_model: Option<String>,
        /// Give it Home Assistant's devices as tools (HOME_ASSISTANT_URL,
        /// HOME_ASSISTANT_TOKEN): their states read, a service called as an
        /// external call.
        #[arg(long)]
        home_assistant: bool,
        /// Tokens each run may use, its subagents' included: before each
        /// model call, what it sends (estimated) and the most it may answer
        /// must fit in what is left, and when they do not, the run ends with
        /// a last answer, without tools, of what it did and what remains.
        #[arg(long, value_name = "TOKENS")]
        budget_tokens: Option<u64>,
        /// Dollars each run may spend, its subagents' included, at the
        /// models' prices (`sion agent prices`). Every model it may use
        /// needs a price.
        #[arg(long, value_name = "DOLLARS", value_parser = crate::agents::parse_usd)]
        budget_usd: Option<u64>,
        /// The most model turns a run may take (by default 32).
        #[arg(long, value_name = "TURNS")]
        max_turns: Option<u32>,
        /// The most tokens each of the model's answers may have (by default
        /// 4,096), not counting room to think.
        #[arg(long, value_name = "TOKENS")]
        max_tokens: Option<u32>,
        /// Compact a conversation once it reaches about this many tokens
        /// (by default about 40,000, or 36 messages): the middle is replaced
        /// by a summary.
        #[arg(long, value_name = "TOKENS")]
        compact_at: Option<u64>,
        /// The model that writes the summary when a conversation is
        /// compacted, or none for a mechanical one (by default the first
        /// model).
        #[arg(long, value_name = "MODEL")]
        summary_model: Option<String>,
        /// Give it no plan to keep (`plan_update`), nor hold it to one.
        #[arg(long)]
        no_plan: bool,
        /// Let it finish after changing code without running anything since.
        #[arg(long)]
        no_change_check: bool,
        /// Hold what tools return from outside out of the conversation: the
        /// agent reads it through a reader without tools, its answers held
        /// to a schema, and the session is not tainted by values.
        #[arg(long)]
        quarantine: bool,
        /// A curated log its sessions keep in mind, read-only (its id, or a
        /// unique prefix of it). Repeat for more.
        #[arg(long = "shared-memory", value_name = "LOG")]
        shared_memory: Vec<String>,
        /// Its sessions hold confidential content from the start: their
        /// external calls and spending wait for a person.
        #[arg(long)]
        confidential: bool,
        /// A path of its workspace whose content is confidential (a file, a
        /// directory with a closing `/`, or a prefix ending in `*`). Repeat
        /// for more.
        #[arg(
            long = "confidential-file",
            value_name = "PATH",
            requires = "workspace"
        )]
        confidential_files: Vec<String>,
    },
    /// The profiles on this machine.
    List,
    /// Remove a profile.
    Remove {
        /// Its name.
        name: String,
    },
    /// A profile's channel tokens: each reaches only that profile, through
    /// the HTTP channel or a gateway, and is kept only as its hash.
    #[command(subcommand)]
    Token(TokenCommand),
}

/// `sion agent profile token …`
#[derive(Debug, clap::Subcommand)]
pub enum TokenCommand {
    /// Make a token for a profile (shown once).
    Add {
        /// The profile (on a gateway, a profile served from elsewhere).
        profile: String,
        /// The profile's agent, for a profile served from another machine:
        /// a gateway routes the profile only to a host that proves this
        /// agent's key. A profile of this machine names its own.
        #[arg(long)]
        agent: Option<String>,
        /// Let it approve and decline what the profile's sessions wait on.
        #[arg(long)]
        approve: bool,
    },
    /// The tokens on this machine, by id (never the tokens themselves).
    List {
        /// Only this profile's.
        profile: Option<String>,
    },
    /// Remove a token.
    Remove {
        /// The profile.
        profile: String,
        /// The token's id, as `list` shows it.
        id: String,
    },
}

fn hex_id(text: &str, what: &str) -> Result<sion_core::hash::Hash32, CliError> {
    sion_core::hash::Hash32::from_hex(text.trim())
        .map_err(|_| CliError::Usage(format!("{what} must be 64 hex digits")))
}

/// Runs a profile command.
pub fn run(ledger: &dyn crate::ledger::Ledger, command: ProfileCommand) -> Result<(), CliError> {
    match command {
        ProfileCommand::Add {
            name,
            wallet,
            agent,
            models,
            autonomy,
            memory,
            skills,
            workspace,
            web,
            delegate,
            subagent_models,
            adjudicator,
            no_reflect,
            shell,
            mcp,
            storage,
            screen,
            instructions,
            no_skill_approvals,
            image_model,
            home_assistant,
            budget_tokens,
            budget_usd,
            max_turns,
            max_tokens,
            compact_at,
            summary_model,
            no_plan,
            no_change_check,
            quarantine,
            shared_memory,
            confidential,
            confidential_files,
        } => {
            let shell = shell.sandbox()?;
            let shared_memory = shared_memory
                .iter()
                .map(|log| crate::memory::find_log(ledger, log))
                .collect::<Result<Vec<_>, _>>()?;
            let screen = screen.screen()?;
            screen.check(workspace.as_deref())?;
            for server in &mcp {
                crate::agent_mcp::get(server)?;
            }
            if let Some(root) = &workspace {
                crate::runtime::check_workspace(root)?;
                crate::runtime::check_apart(root, skills.as_deref())?;
            }
            if let Some(sandbox) = &shell {
                let root = workspace.as_ref().ok_or_else(|| {
                    CliError::Usage("a shell needs a workspace to work in: give --workspace".into())
                })?;
                crate::runtime::check_shell(sandbox, root, skills.as_deref())?;
            }
            if !valid_name(&name) {
                return Err(CliError::Usage(format!(
                    "{name:?} is not a profile name: 1 to 32 lowercase letters, digits and hyphens"
                )));
            }
            let agent = ObjectId::from_hash(hex_id(&agent, "the agent")?);
            // The agent must exist.
            crate::agents::identity(ledger, agent)?;
            let kept = all()?.remove(&name);
            let profile = Profile {
                name: name.clone(),
                wallet,
                agent,
                models,
                autonomy: autonomy.parse().map_err(CliError::Usage)?,
                memory: memory
                    .map(|m| crate::memory::find_log(ledger, &m))
                    .transpose()?,
                skills,
                workspace,
                web,
                delegate,
                subagent_models,
                adjudicator: adjudicator
                    .map(|a| hex_id(&a, "the adjudicator").map(AtomId::from_hash))
                    .transpose()?,
                reflect: !no_reflect,
                shell,
                mcp,
                storage,
                instructions: instructions.filter(|i| !i.trim().is_empty()),
                skill_approvals: !no_skill_approvals,
                screen,
                image_model: match image_model {
                    Some(spec) => {
                        crate::files::image_model(&spec)?;
                        Some(spec)
                    }
                    None => None,
                },
                home_assistant,
                budget_tokens,
                budget_micros: budget_usd,
                max_turns,
                max_tokens,
                compact_at,
                summary_model,
                no_plan,
                no_change_check,
                quarantine,
                shared_memory,
                confidential,
                confidential_files,
                // A profile added again under its name keeps its routes.
                routes: kept.as_ref().map(|k| k.routes.clone()).unwrap_or_default(),
                sorter: kept.and_then(|k| k.sorter),
            };
            if let Some(other) = crate::channels::tokens()?
                .iter()
                .find(|t| t.profile == name && t.agent.is_some_and(|a| a != profile.agent))
            {
                return Err(CliError::Usage(format!(
                    "profile {name}'s token {} is for agent {}: remove it first (sion agent profile token remove {name} {})",
                    other.id,
                    other.agent.map(|a| a.0.short()).unwrap_or_default(),
                    other.id
                )));
            }
            let mut profiles = all()?;
            profiles.insert(name.clone(), profile.clone());
            save_all(&profiles)?;
            println!("{}", describe(&profile));
        }
        ProfileCommand::List => {
            let profiles = all()?;
            if profiles.is_empty() {
                println!("no agent profiles in {}", path().display());
            }
            for profile in profiles.values() {
                println!("{}", describe(profile));
            }
        }
        ProfileCommand::Remove { name } => {
            let mut profiles = all()?;
            if profiles.remove(&name).is_none() {
                return Err(CliError::Usage(format!("no agent profile {name}")));
            }
            save_all(&profiles)?;
            match crate::channels::remove_tokens(&name)? {
                0 => println!("removed profile {name}"),
                1 => println!("removed profile {name} and its channel token"),
                n => println!("removed profile {name} and its {n} channel tokens"),
            }
        }
        ProfileCommand::Token(TokenCommand::Add {
            profile,
            agent,
            approve,
        }) => {
            let named = agent
                .map(|a| hex_id(&a, "--agent").map(sion_core::ids::ObjectId::from_hash))
                .transpose()?;
            let agent = match (all()?.get(&profile).map(|p| p.agent), named) {
                (Some(own), Some(named)) if own != named => {
                    return Err(CliError::Usage(format!(
                        "profile {profile} here is agent {}, not {}",
                        own.0.short(),
                        named.0.short()
                    )))
                }
                (Some(own), _) => own,
                (None, Some(named)) => named,
                (None, None) => {
                    return Err(CliError::Usage(format!(
                        "no profile {profile} on this machine: for one served elsewhere, name its agent with --agent"
                    )))
                }
            };
            let (record, secret) = crate::channels::add_token(&profile, agent, approve)?;
            println!(
                "token {} for profile {profile}, agent {} ({}); it is shown once, and kept here only as its hash:",
                record.id,
                agent.0.short(),
                if approve {
                    "messages, approvals and declines"
                } else {
                    "messages only"
                }
            );
            println!("{secret}");
        }
        ProfileCommand::Token(TokenCommand::List { profile }) => {
            let tokens: Vec<_> = crate::channels::tokens()?
                .into_iter()
                .filter(|t| match &profile {
                    Some(wanted) => *wanted == t.profile,
                    None => true,
                })
                .collect();
            if tokens.is_empty() {
                println!("no channel tokens");
            }
            for token in tokens {
                println!(
                    "{}  {}  {}  {}",
                    token.id,
                    token.profile,
                    token
                        .agent
                        .map(|a| format!("agent {}", a.0.short()))
                        .unwrap_or_else(|| "no agent".to_string()),
                    if token.approve {
                        "messages, approvals"
                    } else {
                        "messages"
                    }
                );
            }
        }
        ProfileCommand::Token(TokenCommand::Remove { profile, id }) => {
            if !crate::channels::remove_token(&profile, &id)? {
                return Err(CliError::Usage(format!(
                    "profile {profile} has no token {id}"
                )));
            }
            println!("removed token {id} of profile {profile}");
        }
    }
    Ok(())
}

/// One line about a profile.
pub fn describe(profile: &Profile) -> String {
    let mut extras = Vec::new();
    if profile.memory.is_some() {
        extras.push("memory".to_string());
    }
    if profile.instructions.is_some() {
        extras.push("instructions".to_string());
    }
    if profile.web {
        extras.push("web".to_string());
    }
    if profile.delegate {
        extras.push("delegates".to_string());
    }
    if let Some(sandbox) = &profile.shell {
        extras.push(format!("a shell ({})", sandbox.describe()));
    }
    if !profile.mcp.is_empty() {
        extras.push(format!("MCP servers {}", profile.mcp.join(", ")));
    }
    if profile.storage {
        extras.push("storage deals".to_string());
    }
    if let Some(tokens) = profile.budget_tokens {
        extras.push(format!("a budget of {tokens} tokens a run"));
    }
    if let Some(micros) = profile.budget_micros {
        extras.push(format!(
            "a budget of {} a run",
            sion_agent::prices::dollars(micros)
        ));
    }
    if let Some(turns) = profile.max_turns {
        extras.push(format!("at most {turns} turns a run"));
    }
    if let Some(tokens) = profile.max_tokens {
        extras.push(format!("answers of at most {tokens} tokens"));
    }
    if let Some(tokens) = profile.compact_at {
        extras.push(format!("compacted at about {tokens} tokens"));
    }
    if let Some(spec) = &profile.summary_model {
        extras.push(format!("summaries by {spec}"));
    }
    if profile.no_plan {
        extras.push("no plan".to_string());
    }
    if profile.no_change_check {
        extras.push("changes not checked".to_string());
    }
    if profile.quarantine {
        extras.push("quarantine".to_string());
    }
    if !profile.routes.is_empty() {
        let names: Vec<&str> = profile.routes.iter().map(|r| r.name.as_str()).collect();
        extras.push(format!("routes {}", names.join(", ")));
    }
    format!(
        "profile {}: agent {} as {}, {} at {:?}{}",
        profile.name,
        profile.agent.short(),
        profile.wallet,
        profile.models.join(", "),
        profile.autonomy,
        if extras.is_empty() {
            String::new()
        } else {
            format!(", with {}", extras.join(", "))
        }
    )
}
