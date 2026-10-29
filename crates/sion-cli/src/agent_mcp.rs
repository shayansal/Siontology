//! MCP servers as agents' tools: `sion agent mcp add|list|review|remove`.
//!
//! Servers are kept in `$SION_HOME/agents/mcp.json` with their reviewed
//! tools, and a session mounts them with `--mcp <name>`: the runtime starts
//! each server for the run and offers only the tools whose definitions
//! still match their review (see `sion_agent::mcp`).

use std::collections::BTreeMap;
use std::path::PathBuf;

use sion_agent::mcp::{self, Connection, Listed, Server};
use sion_agent::Effect;

use crate::error::CliError;
use crate::session::wallet_dir;

/// `sion agent mcp …`
#[derive(Debug, clap::Subcommand)]
// Parsed once from the command line: the size of a variant does not matter.
#[allow(clippy::large_enum_variant)]
pub enum McpCommand {
    /// Add an MCP server for agents' sessions, and review its tools. The
    /// server is started and its tools listed, each with what its calls
    /// are taken to do; those offered are pinned by a digest of their
    /// definitions, so a tool that changes is not offered until reviewed
    /// again.
    Add {
        /// Its name (lowercase letters, digits and hyphens).
        name: String,
        /// An environment variable it is given, by name; the value is the
        /// one where the agent runs. Repeat for more.
        #[arg(long = "env", value_name = "NAME")]
        env: Vec<String>,
        /// The directory it works in (in a sandbox, at /workspace).
        #[arg(long)]
        dir: Option<PathBuf>,
        /// Offer only this tool of its tools (default: all). Repeat for more.
        #[arg(long = "allow", value_name = "TOOL")]
        allow: Vec<String>,
        /// What a tool's calls are taken to do, whatever its annotations
        /// say: TOOL=read, write, external or spend. Repeat for more.
        #[arg(long = "effect", value_name = "TOOL=EFFECT")]
        effects: Vec<String>,
        /// Seconds a call may take.
        #[arg(long, default_value_t = 120)]
        timeout: u64,
        /// The sandbox it runs in: bwrap or docker:<image>@sha256:<digest>.
        /// Without one, it runs as you.
        #[arg(long, value_name = "SANDBOX")]
        sandbox: Option<String>,
        /// The network it reaches in its sandbox: none (the default), host
        /// (bubblewrap) or a Docker network you set up.
        #[arg(long, default_value = "none", value_name = "NETWORK")]
        sandbox_network: String,
        /// A directory it may read in its sandbox. Repeat for more.
        #[arg(long = "sandbox-read", value_name = "DIR")]
        sandbox_read: Vec<PathBuf>,
        /// The program and its arguments, after `--`.
        #[arg(last = true, required = true)]
        command: Vec<String>,
    },
    /// The MCP servers on this machine, with their reviewed tools.
    List,
    /// Review a server's tools again: what changed, what is new and what is
    /// gone since its last review. With --accept, its tools are pinned as
    /// they are now (a tool keeps the larger of its old effect and what its
    /// annotations now say, unless --effect sets one).
    Review {
        /// The server.
        name: String,
        /// Pin the tools as listed.
        #[arg(long)]
        accept: bool,
        /// What a tool's calls are taken to do: TOOL=read, write, external
        /// or spend. Repeat for more.
        #[arg(long = "effect", value_name = "TOOL=EFFECT")]
        effects: Vec<String>,
    },
    /// Remove a server.
    Remove {
        /// The server.
        name: String,
    },
}

/// The file servers are kept in: `$SION_HOME/agents/mcp.json`.
pub fn path() -> PathBuf {
    wallet_dir()
        .parent()
        .map(|home| home.join("agents").join("mcp.json"))
        .unwrap_or_else(|| PathBuf::from(".sion/agents/mcp.json"))
}

/// Every server, by name.
pub fn all() -> Result<BTreeMap<String, Server>, CliError> {
    let path = path();
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|e| CliError::Usage(format!("{}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(CliError::Usage(format!("{}: {e}", path.display()))),
    }
}

/// The server `name`.
pub fn get(name: &str) -> Result<Server, CliError> {
    all()?.remove(name).ok_or_else(|| {
        CliError::Usage(format!(
            "no MCP server {name} (`sion agent mcp add {name} -- <command>`)"
        ))
    })
}

fn save_all(servers: &BTreeMap<String, Server>) -> Result<(), CliError> {
    crate::profiles::write_json(&path(), servers)
}

/// `TOOL=EFFECT` pairs.
fn effects(pairs: &[String]) -> Result<BTreeMap<String, Effect>, CliError> {
    pairs
        .iter()
        .map(|pair| {
            let (tool, effect) = pair.split_once('=').ok_or_else(|| {
                CliError::Usage(format!("{pair}: give TOOL=read, write, external or spend"))
            })?;
            let effect: Effect =
                serde_json::from_value(serde_json::json!(effect.trim())).map_err(|_| {
                    CliError::Usage(format!(
                        "{effect}: an effect is read, write, external or spend"
                    ))
                })?;
            Ok((tool.trim().to_string(), effect))
        })
        .collect()
}

fn effect_name(effect: Effect) -> &'static str {
    match effect {
        Effect::Read => "read",
        Effect::Write => "write",
        Effect::External => "external",
        Effect::Spend => "spend",
    }
}

/// One line about a tool as reviewed.
fn line(tool: &Listed, pin: &mcp::Pin) -> String {
    let mut description: String = tool
        .description
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if description.chars().count() > 160 {
        description = description.chars().take(157).collect::<String>() + "…";
    }
    format!(
        "  {} ({}{}): {description}",
        tool.name,
        effect_name(pin.effect),
        // Reads are never journaled: saying so of them would be noise.
        if pin.journaled || pin.effect == Effect::Read {
            ""
        } else {
            ", repeats run again"
        }
    )
}

/// What the server said about itself, the protocol it speaks, and its tools.
fn listed(server: &Server) -> Result<(serde_json::Value, String, Vec<Listed>), CliError> {
    let mut connection = Connection::start(server).map_err(CliError::Usage)?;
    let tools = connection.tools().map_err(CliError::Usage)?;
    Ok((connection.info.clone(), connection.protocol(), tools))
}

/// Starts `server`, lists its tools and pins them as they are now: those
/// in `allow` (all, if it is empty), with the effects `overrides` sets.
/// Returns what the server said about itself, the tools as listed and the
/// server with its pins.
pub fn review(
    mut server: Server,
    allow: &[String],
    overrides: &BTreeMap<String, Effect>,
) -> Result<(serde_json::Value, Vec<Listed>, Server), CliError> {
    let (info, protocol, tools) = listed(&server)?;
    server.protocol = Some(protocol);
    for wanted in allow.iter().chain(overrides.keys()) {
        if !tools.iter().any(|t| &t.name == wanted) {
            return Err(CliError::Usage(format!(
                "the server has no tool {wanted}; it has {}",
                tools
                    .iter()
                    .map(|t| t.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
    }
    server.tools = tools
        .iter()
        .filter(|t| allow.is_empty() || allow.contains(&t.name))
        .map(|t| (t.name.clone(), t.pin(overrides.get(&t.name).copied())))
        .collect();
    Ok((info, tools, server))
}

/// Keeps `server` (replacing one of its name).
pub fn save(server: Server) -> Result<(), CliError> {
    let mut servers = all()?;
    servers.insert(server.name.clone(), server);
    save_all(&servers)
}

fn about(info: &serde_json::Value, protocol: Option<&str>) -> String {
    let said = match (info["name"].as_str(), info["version"].as_str()) {
        (Some(name), Some(version)) => format!("{name} {version}"),
        (Some(name), None) => name.to_string(),
        _ => String::new(),
    };
    match (said.is_empty(), protocol) {
        (false, Some(protocol)) => format!(" ({said}, protocol {protocol})"),
        (true, Some(protocol)) => format!(" (protocol {protocol})"),
        (false, None) => format!(" ({said})"),
        (true, None) => String::new(),
    }
}

/// Runs an MCP command.
pub fn run(command: McpCommand) -> Result<(), CliError> {
    match command {
        McpCommand::Add {
            name,
            env,
            dir,
            allow,
            effects: pairs,
            timeout,
            sandbox,
            sandbox_network,
            sandbox_read,
            command,
        } => {
            if !mcp::valid_name(&name) {
                return Err(CliError::Usage(format!(
                    "{name:?} is not a server name: 1 to 32 lowercase letters, digits and hyphens"
                )));
            }
            let sandbox = match sandbox {
                Some(backend) => Some(crate::runtime::sandbox_from(
                    &backend,
                    &sandbox_network,
                    sandbox_read,
                    timeout,
                )?),
                None => {
                    if sandbox_network != "none" || !sandbox_read.is_empty() {
                        return Err(CliError::Usage(
                            "--sandbox-network and --sandbox-read go with --sandbox".into(),
                        ));
                    }
                    None
                }
            };
            let dir = dir.map(|d| crate::runtime::resolved(&d));
            if let Some(dir) = &dir {
                crate::runtime::check_workspace(dir)?;
                if !dir.is_dir() {
                    return Err(CliError::Usage(format!(
                        "{} is not a directory",
                        dir.display()
                    )));
                }
            }
            if let Some(sandbox) = &sandbox {
                if let Some(dir) = &dir {
                    crate::runtime::check_shell(sandbox, dir, None)?;
                }
                sandbox.probe().map_err(|e| {
                    CliError::Usage(format!("the sandbox cannot run the server here: {e}"))
                })?;
            }
            let overrides = effects(&pairs)?;
            let server = Server {
                name: name.clone(),
                command,
                env,
                sandbox,
                dir,
                tools: BTreeMap::new(),
                timeout_secs: timeout.max(1),
                protocol: None,
            };
            let (info, tools, server) = review(server, &allow, &overrides)?;
            println!(
                "MCP server {name}{}: reviewed tools",
                about(&info, server.protocol.as_deref())
            );
            for tool in &tools {
                if let Some(pin) = server.tools.get(&tool.name) {
                    println!("{}", line(tool, pin));
                }
            }
            if server.sandbox.is_none() {
                println!(
                    "It runs as you, outside any sandbox: add --sandbox bwrap (or a Docker image) to contain it."
                );
            }
            save(server)?;
            println!("Give it to a session with --mcp {name}.");
        }
        McpCommand::List => {
            let servers = all()?;
            if servers.is_empty() {
                println!("no MCP servers in {}", path().display());
            }
            for server in servers.values() {
                println!(
                    "{}: {}{}{}",
                    server.name,
                    server.command.join(" "),
                    match &server.sandbox {
                        Some(sandbox) => format!(", in {}", sandbox.describe()),
                        None => ", unsandboxed".to_string(),
                    },
                    if server.env.is_empty() {
                        String::new()
                    } else {
                        format!(", given {}", server.env.join(", "))
                    }
                );
                for (tool, pin) in &server.tools {
                    println!("  {tool} ({})", effect_name(pin.effect));
                }
            }
        }
        McpCommand::Review {
            name,
            accept,
            effects: pairs,
        } => {
            let mut servers = all()?;
            let server = servers
                .get_mut(&name)
                .ok_or_else(|| CliError::Usage(format!("no MCP server {name}")))?;
            let overrides = effects(&pairs)?;
            let (info, protocol, tools) = listed(server)?;
            println!("MCP server {name}{}:", about(&info, Some(&protocol)));
            let mut pins = BTreeMap::new();
            for tool in &tools {
                let old = server.tools.get(&tool.name);
                let state = match old {
                    Some(pin) if pin.digest == tool.digest => "unchanged",
                    Some(_) => "changed",
                    None => "new",
                };
                let effect = match (overrides.get(&tool.name), old) {
                    (Some(effect), _) => *effect,
                    (None, Some(pin)) => pin.effect.max(tool.effect()),
                    (None, None) => tool.effect(),
                };
                let pin = tool.pin(Some(effect));
                println!("{} [{state}]", line(tool, &pin));
                pins.insert(tool.name.clone(), pin);
            }
            for gone in server.tools.keys().filter(|k| !pins.contains_key(*k)) {
                println!("  {gone} [gone]");
            }
            if accept {
                server.tools = pins;
                server.protocol = Some(protocol);
                save_all(&servers)?;
                println!("Pinned as listed.");
            } else {
                println!("Nothing changed: add --accept to pin the tools as listed.");
            }
        }
        McpCommand::Remove { name } => {
            let mut servers = all()?;
            if servers.remove(&name).is_none() {
                return Err(CliError::Usage(format!("no MCP server {name}")));
            }
            save_all(&servers)?;
            println!("removed MCP server {name}");
        }
    }
    Ok(())
}
