//! MCP servers as agents' tools: `sion agent mcp
//! add|list|review|login|logout|remove`.
//!
//! Servers are kept in `$SION_HOME/agents/mcp.json` with their reviewed
//! tools, and a session mounts them with `--mcp <name>`: the runtime starts
//! each server (or reaches it, over HTTP) for the run and offers only the
//! tools whose definitions still match their review (see
//! `sion_agent::mcp`). A server over HTTP that asks for authorization is
//! logged in to with OAuth ([`crate::mcp_auth`]).

use std::collections::BTreeMap;
use std::path::PathBuf;

use std::sync::Arc;

use sion_agent::mcp::{self, Connection, Listed, Server};
use sion_agent::oauth::Authorizer;
use sion_agent::Effect;

use crate::mcp_auth::{AuthStore, OAuthConfig};

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
        /// Reach it over HTTP at this endpoint (Streamable HTTP, or the
        /// deprecated HTTP+SSE), instead of starting a program.
        #[arg(long, conflicts_with_all = ["env", "dir", "sandbox", "sandbox_read"])]
        url: Option<String>,
        /// A header sent to a server over HTTP, `Name: value`; `${VAR}` in
        /// the value is the environment variable's value where the agent
        /// runs, so the secret itself is not kept. Repeat for more.
        #[arg(long = "header", value_name = "HEADER", requires = "url")]
        headers: Vec<String>,
        #[command(flatten)]
        oauth: OAuthArgs,
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
        /// One of sion's own servers instead of a program: mail (MAIL_…),
        /// calendar (CALDAV_…) or documents (the folder --dir names). The
        /// program and the variables it needs are filled in.
        #[arg(long, value_name = "SERVER", conflicts_with = "url")]
        first_party: Option<String>,
        /// The program and its arguments, after `--`.
        #[arg(last = true, required_unless_present_any = ["url", "first_party"])]
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
    /// Log in to a server over HTTP that asks for authorization (OAuth):
    /// a browser opens where you authorize, and the tokens are kept,
    /// encrypted, for its sessions, which renew them while they can.
    Login {
        /// The server.
        name: String,
        /// Scopes to ask for besides those the server asks for.
        #[arg(long)]
        scope: Option<String>,
        /// Do not open a browser: print the URL to open.
        #[arg(long)]
        no_browser: bool,
    },
    /// Forget a server's authorization (its tokens and registration),
    /// revoking its tokens first where its authorization server takes
    /// revocations.
    Logout {
        /// The server.
        name: String,
    },
    /// Remove a server, and its authorization as `logout` does.
    Remove {
        /// The server.
        name: String,
    },
    /// Let a server have the session's model write for it (sampling),
    /// within limits: at most --max-tokens a completion and --per-run
    /// completions a run, written by --model (default: the session's
    /// first). The completions are counted in the run's token budget. With
    /// --off, it may not. Sampling is deprecated by protocol 2026-07-28.
    Sampling {
        /// The server.
        name: String,
        /// The most tokens one completion may write.
        #[arg(long, default_value_t = 1000)]
        max_tokens: u32,
        /// The most completions in one run of a session.
        #[arg(long, default_value_t = 5)]
        per_run: u32,
        /// The model that writes them.
        #[arg(long)]
        model: Option<String>,
        /// Take sampling away.
        #[arg(long)]
        off: bool,
    },
    /// The resources a server offers, and its templates; with --read, what
    /// one holds.
    Resources {
        /// The server.
        name: String,
        /// The address of a resource to read.
        #[arg(long, value_name = "URI")]
        read: Option<String>,
    },
    /// The prompts a server offers, which people run in chats with
    /// `/prompt <server> <prompt> name=value …`.
    Prompts {
        /// The server.
        name: String,
    },
}

/// The variables each of sion's own MCP servers reads.
pub const FIRST_PARTY: [(&str, &[&str]); 3] = [
    (
        "mail",
        &[
            "MAIL_ADDRESS",
            "MAIL_IMAP_URL",
            "MAIL_SMTP_URL",
            "MAIL_PASSWORD",
            "MAIL_TOKEN_COMMAND",
            "MAIL_USERNAME",
            "MAIL_NAME",
            "MAIL_MAILBOX",
        ],
    ),
    (
        "calendar",
        &[
            "CALDAV_URL",
            "CALDAV_USERNAME",
            "CALDAV_PASSWORD",
            "CALDAV_TOKEN_COMMAND",
        ],
    ),
    ("documents", &[]),
];

/// The variables and the command line of sion's own server `kind`, the
/// variables `env` names besides: this program, `mcp <kind>`, the home
/// whose kept credentials it reads, and for documents the folder (its
/// place in the sandbox, when it runs in one).
pub fn first_party_server(
    kind: &str,
    dir: Option<&std::path::Path>,
    sandboxed: bool,
    env: Vec<String>,
) -> Result<(Vec<String>, Vec<String>), CliError> {
    let Some((_, names)) = FIRST_PARTY.iter().find(|(k, _)| *k == kind) else {
        return Err(CliError::Usage(format!(
            "{kind} is not one of sion's servers: mail, calendar or documents"
        )));
    };
    let program = std::env::current_exe()
        .map_err(|e| CliError::Usage(format!("where this program is: {e}")))?
        .to_string_lossy()
        .into_owned();
    let mut command = vec![program, "mcp".into(), kind.into()];
    if kind == "documents" {
        let dir = dir.ok_or_else(|| {
            CliError::Usage("the documents server needs its folder: --dir".into())
        })?;
        command.push("--dir".into());
        command.push(if sandboxed {
            sion_agent::sandbox::WORKSPACE.to_string()
        } else {
            crate::runtime::resolved(dir).to_string_lossy().into_owned()
        });
    }
    let mut all: Vec<String> = ["HOME", "SION_HOME"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    all.extend(names.iter().map(|s| s.to_string()));
    for name in env {
        if !all.contains(&name) {
            all.push(name);
        }
    }
    Ok((all, command))
}

/// A server's model for sampling: the operator's limits, counted for one
/// run of a session.
pub struct ModelSampler {
    limits: mcp::Sampling,
    models: Vec<String>,
    used: std::sync::atomic::AtomicU32,
}

impl std::fmt::Debug for ModelSampler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelSampler")
            .field("limits", &self.limits)
            .finish()
    }
}

impl ModelSampler {
    /// Sampling within `limits`, written by their model or else the first
    /// of `models` that answers.
    pub fn new(limits: &mcp::Sampling, models: &[String]) -> ModelSampler {
        ModelSampler {
            limits: limits.clone(),
            models: match &limits.model {
                Some(model) => vec![model.clone()],
                None => models.to_vec(),
            },
            used: std::sync::atomic::AtomicU32::new(0),
        }
    }
}

/// The most characters a server's messages may hold, for the operator's
/// model to write after.
pub const MAX_SAMPLED: usize = 100_000;

impl sion_agent::mcp_input::Sampler for ModelSampler {
    fn sample(
        &self,
        server: &str,
        params: &serde_json::Value,
    ) -> Result<(serde_json::Value, sion_agent::model::Usage), String> {
        use sion_agent::model::{Block, Message, Request, Role, Stop};
        if self.used.fetch_add(1, std::sync::atomic::Ordering::SeqCst) >= self.limits.per_run {
            return Err(format!(
                "{server} has sampled {} times in this run, the most it may",
                self.limits.per_run
            ));
        }
        let mut messages = Vec::new();
        for message in params["messages"].as_array().into_iter().flatten() {
            let role = match message["role"].as_str() {
                Some("assistant") => Role::Assistant,
                _ => Role::User,
            };
            let parts: Vec<&serde_json::Value> = match &message["content"] {
                serde_json::Value::Array(items) => items.iter().collect(),
                item => vec![item],
            };
            let text: Vec<String> = parts
                .iter()
                .map(|part| match part["type"].as_str() {
                    Some("text") => part["text"].as_str().unwrap_or_default().to_string(),
                    _ => "[a picture or sound, left out]".to_string(),
                })
                .collect();
            messages.push(Message {
                role,
                content: vec![Block::text(text.join("\n"))],
            });
        }
        if messages.is_empty() {
            return Err("the server asked for a completion of nothing".into());
        }
        // What the operator's model is given to read is bounded too.
        let given: usize = messages
            .iter()
            .flat_map(|m| &m.content)
            .map(|b| match b {
                Block::Text { text } => text.chars().count(),
                _ => 0,
            })
            .sum::<usize>()
            + params["systemPrompt"]
                .as_str()
                .map_or(0, |s| s.chars().count());
        if given > MAX_SAMPLED {
            return Err(format!(
                "{server} asked for a completion of {given} characters; at most {MAX_SAMPLED} \
                 are written for a server"
            ));
        }
        let max_tokens = params["maxTokens"]
            .as_u64()
            .map(|n| n.min(u64::from(self.limits.max_tokens)) as u32)
            .unwrap_or(self.limits.max_tokens)
            .max(1);
        let request = Request {
            system: params["systemPrompt"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            messages,
            tools: Vec::new(),
            max_tokens,
            images: Default::default(),
        };
        let mut failed = String::from("no model to write with");
        for spec in &self.models {
            let model = match crate::runtime::model(spec, 0) {
                Ok(model) => model,
                Err(e) => {
                    failed = e.to_string();
                    continue;
                }
            };
            match model.complete(&request) {
                Ok(response) => {
                    let text: String = response
                        .content
                        .iter()
                        .filter_map(|b| match b {
                            Block::Text { text } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("");
                    let stop = match response.stop {
                        Stop::MaxTokens => "maxTokens",
                        _ => "endTurn",
                    };
                    return Ok((
                        serde_json::json!({
                            "role": "assistant",
                            "content": {"type": "text", "text": text},
                            "model": response.model,
                            "stopReason": stop,
                        }),
                        response.usage,
                    ));
                }
                Err(e) => failed = e.to_string(),
            }
        }
        Err(format!("the model could not write it: {failed}"))
    }
}

/// The prompts of the MCP servers `names`, one line each, for people.
pub fn prompts_text(names: &[String]) -> String {
    if names.is_empty() {
        return "This agent has no MCP servers.".into();
    }
    let mut lines = Vec::new();
    for name in names {
        let found = get(name).map_err(|e| e.to_string()).and_then(|server| {
            let auth = authorizer(&server).map_err(|e| e.to_string())?;
            let mut connection = Connection::start_with(&server, auth)?;
            if !connection.offers("prompts") {
                return Ok(Vec::new());
            }
            connection.prompts().map_err(|e| e.to_string())
        });
        match found {
            Ok(prompts) => {
                for prompt in prompts {
                    let arguments: Vec<String> = prompt["arguments"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|a| {
                            let n = a["name"].as_str()?;
                            Some(if a["required"].as_bool().unwrap_or(false) {
                                format!("{n}=…")
                            } else {
                                format!("[{n}=…]")
                            })
                        })
                        .collect();
                    lines.push(format!(
                        "/prompt {name} {} {}{}",
                        prompt["name"].as_str().unwrap_or_default(),
                        arguments.join(" "),
                        prompt["description"]
                            .as_str()
                            .or(prompt["title"].as_str())
                            .map(|d| format!(" — {d}"))
                            .unwrap_or_default()
                    ));
                }
            }
            Err(e) => lines.push(format!("{name}: its prompts could not be listed ({e})")),
        }
    }
    if lines.is_empty() {
        "None of this agent's MCP servers offers prompts.".into()
    } else {
        shown(&lines.join("\n"))
    }
}

/// A server's words as they are shown: its control characters (a
/// terminal's escape sequences, a carriage return that writes over a line)
/// as `�`, its lines and tabs kept.
pub fn shown(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_control() && c != '\n' && c != '\t' {
                '\u{FFFD}'
            } else {
                c
            }
        })
        .collect()
}

/// The message a prompt makes: `rest` is `<server> <prompt> name=value …`,
/// the server one of `names`. Its text is the server's, said as the
/// person's request.
pub fn prompt_text(
    names: &[String],
    rest: &str,
    asker: Option<&Arc<dyn sion_agent::mcp_input::Asker>>,
) -> Result<String, String> {
    let mut words = rest.split_whitespace();
    let (Some(server), Some(prompt)) = (words.next(), words.next()) else {
        return Err("Say /prompt <server> <prompt> name=value …; /prompts lists them.".into());
    };
    if !names.iter().any(|n| n == server) {
        return Err(format!(
            "This agent has no MCP server {server}: /prompts lists its prompts."
        ));
    }
    let mut arguments = serde_json::Map::new();
    for word in words {
        let Some((key, value)) = word.split_once('=') else {
            return Err(format!("{word} is not name=value"));
        };
        arguments.insert(
            key.to_string(),
            serde_json::Value::String(value.to_string()),
        );
    }
    let config = get(server).map_err(|e| e.to_string())?;
    let auth = authorizer(&config).map_err(|e| e.to_string())?;
    let responder = sion_agent::mcp_input::Responder {
        asker: asker.cloned(),
        ..Default::default()
    };
    let mut connection = Connection::start_full(&config, auth, responder)?;
    let got = connection
        .prompt(prompt, &arguments)
        .map_err(|e| format!("The MCP server {server} gave no prompt {prompt}: {e}"))?;
    let mut text = format!("(the prompt {prompt} of the MCP server {server})");
    for message in got["messages"].as_array().into_iter().flatten() {
        let said = match message["content"]["type"].as_str() {
            Some("text") => message["content"]["text"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            Some("resource") => message["content"]["resource"]["text"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            _ => "[a picture or sound, left out]".to_string(),
        };
        if message["role"] == "assistant" {
            text.push_str(&format!("\n(answered before: {said})"));
        } else {
            text.push_str(&format!("\n{said}"));
        }
    }
    if text.len() > crate::channels::MAX_MESSAGE {
        return Err("The prompt is longer than a message may be.".into());
    }
    Ok(text)
}

/// How a server over HTTP is authorized with OAuth.
#[derive(Clone, Debug, Default, clap::Args)]
pub struct OAuthArgs {
    /// A client id registered with its authorization server (default:
    /// registering one, or the client ID metadata document).
    #[arg(long, requires = "url")]
    client_id: Option<String>,
    /// The environment variable holding that client's secret.
    #[arg(long, requires = "client_id")]
    client_secret_env: Option<String>,
    /// The HTTPS URL of a client ID metadata document to use as the client
    /// id, where the authorization server takes one.
    #[arg(long, requires = "url")]
    client_metadata_url: Option<String>,
    /// The loopback port your browser comes back to (default: a free one).
    #[arg(long, requires = "url")]
    callback_port: Option<u16>,
    /// The authorization server to use, when the server names several.
    #[arg(long, requires = "url")]
    issuer: Option<String>,
    /// Scopes to ask for besides those the server asks for.
    #[arg(long, requires = "url")]
    scope: Option<String>,
    /// Authorize the client itself (the client credentials grant), with no
    /// person: for agents that run unattended. Needs --client-id and
    /// --client-secret-env.
    #[arg(long, requires_all = ["client_id", "client_secret_env"])]
    client_credentials: bool,
    /// Do not open a browser: print the URL to open.
    #[arg(long, requires = "url")]
    no_browser: bool,
}

impl OAuthArgs {
    fn config(&self) -> Option<OAuthConfig> {
        let config = OAuthConfig {
            client_id: self.client_id.clone(),
            client_secret_env: self.client_secret_env.clone(),
            client_metadata_url: self.client_metadata_url.clone(),
            callback_port: self.callback_port,
            issuer: self.issuer.clone(),
            scope: self.scope.clone(),
            client_credentials: self.client_credentials,
        };
        (config != OAuthConfig::default()).then_some(config)
    }
}

/// The authorization a session sends `server`, if it is over HTTP and has
/// been logged in to (or is configured for OAuth).
pub fn authorizer(server: &Server) -> Result<Option<Arc<dyn Authorizer>>, CliError> {
    if server.url.is_none() {
        return Ok(None);
    }
    let store = AuthStore::here();
    if server.oauth.is_none() && store.get(&server.name)?.is_none() {
        return Ok(None);
    }
    Ok(Some(Arc::new(crate::mcp_auth::Stored::new(
        server,
        server.oauth.clone().unwrap_or_default(),
        store,
    ))))
}

/// Logs in to `server`, showing or opening the URL to authorize at. A
/// browser on another machine cannot come back to this one: the address it
/// was sent to can be pasted instead.
fn login(server: &Server, scope: Option<&str>, no_browser: bool) -> Result<(), CliError> {
    let config = server.oauth.clone().unwrap_or_default();
    let show = |url: &str| {
        if !no_browser && crate::mcp_auth::open_browser(url) {
            println!("Authorize in the browser that opened, or open:\n  {url}");
        } else {
            println!(
                "Open this URL to authorize:\n  {url}\n\
                 A browser on another machine cannot come back to this one: paste here the address it was sent to."
            );
        }
    };
    let (paste, pasted) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lines() {
            let Ok(line) = line else { break };
            if paste.send(line).is_err() {
                break;
            }
        }
    });
    let lines = crate::mcp_auth::login_with(
        server,
        &config,
        &AuthStore::here(),
        scope,
        &show,
        Some(&pasted),
    )?;
    for line in lines {
        println!("{line}");
    }
    Ok(())
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

/// What the server said about itself, the protocol it speaks (with
/// whether over the deprecated HTTP+SSE), and its tools.
fn listed(server: &Server) -> Result<(serde_json::Value, String, bool, Vec<Listed>), CliError> {
    let mut connection =
        Connection::start_with(server, authorizer(server)?).map_err(CliError::Usage)?;
    let tools = connection.tools().map_err(CliError::Usage)?;
    for why in &connection.rejected {
        println!("not offered: {why}");
    }
    Ok((
        connection.info.clone(),
        connection.protocol(),
        connection.transport() == "http+sse",
        tools,
    ))
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
    let (info, protocol, sse, tools) = listed(&server)?;
    server.protocol = Some(protocol);
    server.legacy_sse = sse;
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
            url,
            headers,
            oauth,
            env,
            dir,
            allow,
            effects: pairs,
            timeout,
            sandbox,
            sandbox_network,
            sandbox_read,
            first_party,
            command,
        } => {
            let (env, command) = match first_party.as_deref() {
                None => (env, command),
                Some(kind) => {
                    if !command.is_empty() {
                        return Err(CliError::Usage(
                            "give either --first-party or a command after --, not both".into(),
                        ));
                    }
                    first_party_server(kind, dir.as_deref(), sandbox.is_some(), env)?
                }
            };
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
            if let Some(url) = &url {
                if !command.is_empty() {
                    return Err(CliError::Usage(
                        "give either --url or a command after --, not both".into(),
                    ));
                }
                if !url.starts_with("https://") && !url.starts_with("http://") {
                    return Err(CliError::Usage(format!(
                        "{url} is not an http or https URL"
                    )));
                }
                if url.starts_with("http://") && !sion_agent::oauth::is_loopback(url) {
                    println!(
                        "{url} is plain HTTP: what passes, tokens included, can be read on the way."
                    );
                }
                sion_agent::mcp_http::configured_headers(&name, &headers)
                    .map_err(CliError::Usage)?;
            }
            let server = Server {
                name: name.clone(),
                command,
                url: url.clone(),
                headers,
                oauth: oauth.config(),
                legacy_sse: false,
                env,
                sandbox,
                dir,
                tools: BTreeMap::new(),
                timeout_secs: timeout.max(1),
                protocol: None,
                sampling: None,
            };
            // A server over HTTP that asks for authorization is logged in
            // to first, if it has not been.
            if let Some(url) = &url {
                let headers = sion_agent::mcp_http::configured_headers(&name, &server.headers)
                    .map_err(CliError::Usage)?;
                if AuthStore::here().get(&name)?.is_none()
                    && crate::mcp_auth::first_challenge(url, &headers)
                        .map_err(CliError::Usage)?
                        .is_some()
                {
                    println!("MCP server {name} asks for authorization.");
                    login(&server, None, oauth.no_browser)?;
                }
            }
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
            if server.sandbox.is_none() && server.url.is_none() {
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
            let store = AuthStore::here();
            for server in servers.values() {
                match &server.url {
                    Some(url) => {
                        let names: Vec<&str> = server
                            .headers
                            .iter()
                            .filter_map(|h| h.split_once(':').map(|(n, _)| n.trim()))
                            .collect();
                        let auth = match store.get(&server.name)? {
                            Some(auth) => format!(", {}", crate::mcp_auth::describe(&auth)),
                            None => String::new(),
                        };
                        println!(
                            "{}: {url} over {}{}{auth}",
                            server.name,
                            if server.legacy_sse {
                                "HTTP+SSE"
                            } else {
                                "HTTP"
                            },
                            if names.is_empty() {
                                String::new()
                            } else {
                                format!(", with headers {}", names.join(", "))
                            }
                        );
                    }
                    None => println!(
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
                    ),
                }
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
            let (info, protocol, sse, tools) = listed(server)?;
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
                server.legacy_sse = sse;
                save_all(&servers)?;
                println!("Pinned as listed.");
            } else {
                println!("Nothing changed: add --accept to pin the tools as listed.");
            }
        }
        McpCommand::Login {
            name,
            scope,
            no_browser,
        } => {
            let server = get(&name)?;
            login(&server, scope.as_deref(), no_browser)?;
        }
        McpCommand::Logout { name } => {
            let server = get(&name)?;
            let config = server.oauth.clone().unwrap_or_default();
            match crate::mcp_auth::logout(&server, &config, &AuthStore::here())? {
                Some(lines) => {
                    for line in lines {
                        println!("{line}");
                    }
                }
                None => println!("MCP server {name} had no authorization to forget"),
            }
        }
        McpCommand::Remove { name } => {
            let mut servers = all()?;
            let removed = servers.remove(&name);
            if removed.is_none() {
                return Err(CliError::Usage(format!("no MCP server {name}")));
            }
            save_all(&servers)?;
            if let Some(server) = removed.as_ref() {
                let config = server.oauth.clone().unwrap_or_default();
                for line in crate::mcp_auth::logout(server, &config, &AuthStore::here())?
                    .unwrap_or_default()
                {
                    println!("{line}");
                }
            }
            println!("removed MCP server {name}");
        }
        McpCommand::Sampling {
            name,
            max_tokens,
            per_run,
            model,
            off,
        } => {
            let mut servers = all()?;
            let server = servers
                .get_mut(&name)
                .ok_or_else(|| CliError::Usage(format!("no MCP server {name}")))?;
            if off {
                server.sampling = None;
                save_all(&servers)?;
                println!("MCP server {name} may not sample");
            } else {
                if let Some(spec) = &model {
                    crate::runtime::model(spec, 0)?;
                }
                server.sampling = Some(mcp::Sampling {
                    max_tokens: max_tokens.max(1),
                    per_run,
                    model: model.clone(),
                });
                save_all(&servers)?;
                println!(
                    "MCP server {name} may sample: at most {} tokens a completion, {per_run} a run, by {}",
                    max_tokens.max(1),
                    model.as_deref().unwrap_or("the session's model")
                );
            }
        }
        McpCommand::Resources { name, read } => {
            let server = get(&name)?;
            let mut connection =
                Connection::start_with(&server, authorizer(&server)?).map_err(CliError::Usage)?;
            if !connection.offers("resources") {
                println!("MCP server {name} offers no resources");
                return Ok(());
            }
            match read {
                Some(uri) => {
                    let result = connection
                        .read_resource(&uri)
                        .map_err(|e| CliError::Usage(e.to_string()))?;
                    println!("{}", shown(&mcp::render_contents(&result)));
                }
                None => {
                    let (resources, templates) = connection
                        .resources()
                        .map_err(|e| CliError::Usage(e.to_string()))?;
                    for r in resources {
                        println!(
                            "{}",
                            shown(&format!(
                                "{} {}",
                                r["uri"].as_str().unwrap_or_default(),
                                r["title"]
                                    .as_str()
                                    .or(r["name"].as_str())
                                    .unwrap_or_default()
                            ))
                        );
                    }
                    for t in templates {
                        println!(
                            "{}",
                            shown(&format!(
                                "{} {} (a template)",
                                t["uriTemplate"].as_str().unwrap_or_default(),
                                t["name"].as_str().unwrap_or_default()
                            ))
                        );
                    }
                }
            }
        }
        McpCommand::Prompts { name } => {
            get(&name)?;
            println!("{}", prompts_text(&[name]));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_servers_words_are_shown_without_control_characters() {
        assert_eq!(
            shown("ok\r\u{1b}[2Kfaked\n\tline"),
            "ok\u{FFFD}\u{FFFD}[2Kfaked\n\tline"
        );
        assert_eq!(shown("plain"), "plain");
    }
}
