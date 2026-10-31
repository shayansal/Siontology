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
}

fn yes() -> bool {
    true
}

impl Profile {
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
            instructions: self.instructions.clone(),
            screen: self.screen.clone(),
            chat_instructions: None,
            only_tools: None,
            image_model: self.image_model.clone(),
        }
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
        let mut models = self.models.clone();
        if let Some(at) = settings
            .model
            .as_ref()
            .and_then(|chosen| models.iter().position(|spec| spec == chosen))
        {
            let chosen = models.remove(at);
            models.insert(0, chosen);
        }
        crate::runtime::start_watched(
            ledger,
            &self.wallet,
            self.agent,
            task,
            pictures,
            models,
            Policy {
                autonomy: self.autonomy,
                skill_approvals: self.skill_approvals,
                ..Policy::default()
            },
            crate::runtime::Equipment {
                chat_instructions: settings.instructions.clone(),
                only_tools: settings.tools.clone(),
                ..self.equipment()
            },
            hooks,
        )
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
        } => {
            let shell = shell.sandbox()?;
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
