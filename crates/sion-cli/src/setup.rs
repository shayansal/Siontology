//! One person, one machine: `sion setup` makes what an agent of one's own
//! needs, and bare `sion` talks with it.
//!
//! Each step is skipped when it is done already, so setup may be run again
//! (to change the model, say):
//!
//! 1. **A chain of one's own**: one validator (`sion devnet`) serving its API
//!    at `127.0.0.1:8730`, its state in `$SION_HOME/chain`, pacing itself
//!    when it has nothing to order (`--idle-round-ms`) so that at rest it
//!    uses little CPU. The user's service manager runs it (a systemd user
//!    unit, or a launchd agent), so that it starts at each login; where
//!    there is none, setup prints the command to run in a terminal of its
//!    own and goes on when the chain answers. A chain that answers already
//!    at the node's address is used as it is. It is a devnet: its money is
//!    test dollars from its faucet, and its validator's keys are derived
//!    from public names, which is safe only because nothing but this
//!    machine reaches it.
//! 2. **Two wallets**: the person's, and the agent's (`<name>-agent`), with
//!    test dollars from the faucet: the person's funds the agent's budget,
//!    the agent's pays its transactions' fees.
//! 3. **The agent**, registered by the person: a budget of 1000 USD, at most
//!    100 USD a payment, a lifetime of a billion Pulses.
//! 4. **Its memory log**.
//! 5. **A model**: Claude, OpenAI or Gemini with an API key, a ChatGPT plan
//!    signed in with the browser, or a model server on this machine or its
//!    network (Ollama, LM Studio, llama.cpp: `local:<model>`). Keys are kept
//!    sealed ([`crate::credentials`]), and setup checks that the model
//!    answers.
//! 6. **A profile** (`assistant` unless named otherwise): the agent, its
//!    model and memory, a workspace it may read and write
//!    (`~/sion-workspace`), the web, and autonomy L5: it asks before it
//!    spends, and before it reaches outside once it has read something
//!    from outside.
//! 7. **Telegram**, if asked for: a service running the bridge with the
//!    bot's token, which gives whoever writes to it first a pairing code to
//!    approve.
//!
//! What setup made is kept in `$SION_HOME/setup.json`; bare `sion` reads it
//! to find the profile it talks with.

use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sion_core::amount::Amount;
use sion_core::ids::{AssetId, ObjectId};
use sion_core::Hash32;

use crate::credentials::Credentials;
use crate::error::CliError;
use crate::ledger::{HttpLedger, Ledger, LedgerExt};
use crate::service::Manager;

/// The API a chain of one's own serves (the CLI's default node).
pub const CHAIN_API: &str = "127.0.0.1:8730";
/// How long the chain waits between rounds while it has nothing to order.
/// Measured on a 2-CPU machine: at rest it used 5.7% of one core (99.5%
/// without the wait), and a transfer still took 2.3 s.
pub const IDLE_ROUND_MS: u64 = 10_000;
/// The agent's lifetime, in Pulses: decades at the chain's pace.
pub const LIFETIME_PULSES: u64 = 1_000_000_000;
/// The model offered first: Claude.
pub const DEFAULT_MODEL: &str = "anthropic:claude-sonnet-5-5";
/// Where a model server on this machine answers by default: Ollama's.
pub const LOCAL_MODEL_URL: &str = "http://127.0.0.1:11434/v1";

/// `sion setup`'s options. Without `--yes`, at a terminal, setup asks for
/// what they leave open.
#[derive(Clone, Debug, Default, clap::Args)]
pub struct SetupArgs {
    /// Your wallet's name (default: your login name).
    #[arg(long)]
    pub name: Option<String>,
    /// The profile to make, which bare `sion` talks with.
    #[arg(long, default_value = "assistant")]
    pub profile: String,
    /// Its model: anthropic:<model>, openai-responses:<model>,
    /// gemini:<model>, chatgpt:<model> or local:<model>.
    #[arg(long)]
    pub model: Option<String>,
    /// Take the model's key from this environment variable.
    #[arg(long, value_name = "VARIABLE")]
    pub key_from_env: Option<String>,
    /// For local:<model>, where its server answers (OpenAI's API): Ollama
    /// http://127.0.0.1:11434/v1, LM Studio http://127.0.0.1:1234/v1,
    /// llama.cpp http://127.0.0.1:8080/v1.
    #[arg(long, value_name = "URL")]
    pub local_url: Option<String>,
    /// The directory it may read and write (default: ~/sion-workspace).
    #[arg(long)]
    pub workspace: Option<PathBuf>,
    /// How much it may do without asking, L0 to L6.
    #[arg(long, default_value = "L5")]
    pub autonomy: String,
    /// How long the chain waits between rounds while it has nothing to
    /// order, in milliseconds.
    #[arg(long, default_value_t = IDLE_ROUND_MS, value_name = "MS")]
    pub idle_round_ms: u64,
    /// Do not install the chain as a service: print the command that runs
    /// it, and wait for it.
    #[arg(long)]
    pub no_service: bool,
    /// Also answer on Telegram (the bot's token is asked for, or taken from
    /// TELEGRAM_BOT_TOKEN).
    #[arg(long)]
    pub telegram: bool,
    /// Do not check that the model answers.
    #[arg(long)]
    pub no_check: bool,
    /// Ask nothing: take the defaults and these options.
    #[arg(long, short = 'y')]
    pub yes: bool,
}

/// What setup made, kept in `$SION_HOME/setup.json`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Made {
    /// The chain's API, if setup made the chain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain: Option<String>,
    /// Whether a service of the user's runs the chain.
    #[serde(default)]
    pub service: bool,
    /// The person's wallet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wallet: Option<String>,
    /// The agent's wallet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_wallet: Option<String>,
    /// The agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<ObjectId>,
    /// Its memory log.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<ObjectId>,
    /// The profile bare `sion` talks with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// Whether a Telegram bot answers for it.
    #[serde(default)]
    pub telegram: bool,
}

fn usage(e: impl std::fmt::Display) -> CliError {
    CliError::Usage(e.to_string())
}

/// `$SION_HOME`.
pub fn home() -> PathBuf {
    crate::session::wallet_dir()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(".sion"))
}

/// Where what setup made is kept.
pub fn path() -> PathBuf {
    home().join("setup.json")
}

/// What setup made on this machine (nothing, if it has not run).
pub fn made() -> Made {
    std::fs::read_to_string(path())
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save(made: &Made) -> Result<(), CliError> {
    crate::profiles::write_json(&path(), made)
}

/// The profile bare `sion` talks with: the one setup made, or the only one
/// there is.
pub fn default_profile() -> Result<String, CliError> {
    let profiles = crate::profiles::all()?;
    if let Some(name) = made().profile.filter(|name| profiles.contains_key(name)) {
        return Ok(name);
    }
    match profiles.len() {
        0 => Err(usage(
            "nothing is set up on this machine yet: run `sion setup` (`sion --help` lists every command)",
        )),
        1 => Ok(profiles.into_keys().next().expect("one")),
        _ => Err(usage(format!(
            "which profile? {} (talk with one: sion agent talk <profile>; or make one the default: sion setup --profile <profile>)",
            profiles.into_keys().collect::<Vec<_>>().join(", ")
        ))),
    }
}

/// A wallet name from a login name: lowercase letters, digits and hyphens.
pub fn wallet_name(login: &str) -> String {
    let name: String = login
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let name = name.trim_matches('-').to_string();
    if name.is_empty() || name.len() > 32 {
        "me".into()
    } else {
        name
    }
}

/// The address a chain of one's own serves at, from the node's URL: a
/// loopback host and its port (`http://127.0.0.1:8730` → `127.0.0.1:8730`).
pub fn local_address(node: &str) -> Option<String> {
    let rest = node.strip_prefix("http://")?.trim_end_matches('/');
    if rest.contains('/') {
        return None;
    }
    let (host, port) = rest.rsplit_once(':')?;
    port.parse::<u16>().ok()?;
    let host = match host {
        "localhost" => "127.0.0.1",
        "127.0.0.1" | "[::1]" => host,
        _ => return None,
    };
    Some(format!("{host}:{port}"))
}

/// What the chain runs: one validator, its API at `address`, its state in
/// `data`, waiting `idle_round_ms` between rounds while idle.
pub fn chain_args(address: &str, data: &Path, idle_round_ms: u64) -> Vec<String> {
    [
        "devnet",
        "--api",
        address,
        "--validators",
        "1",
        "--shards",
        "1",
        "--seats",
        "1",
        "--idle-round-ms",
        &idle_round_ms.to_string(),
        "--report",
        "0",
        "--data-dir",
        &data.display().to_string(),
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// The agent's id in what `sion agent register` printed (`agent <id>`).
pub fn printed_agent(out: &str) -> Option<ObjectId> {
    printed(out, "agent ")
}

/// The log's id in what `sion memory open` printed (`log <id>`).
pub fn printed_log(out: &str) -> Option<ObjectId> {
    printed(out, "log ")
}

fn printed(out: &str, prefix: &str) -> Option<ObjectId> {
    out.lines().find_map(|line| {
        let hex = line.strip_prefix(prefix)?.trim();
        (hex.len() == 64)
            .then(|| Hash32::from_hex(hex).ok().map(ObjectId::from_hash))
            .flatten()
    })
}

/// A model's provider, and what it needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    /// Claude: ANTHROPIC_API_KEY.
    Anthropic,
    /// OpenAI's Responses API: OPENAI_API_KEY.
    OpenAi,
    /// Gemini: GEMINI_API_KEY.
    Gemini,
    /// A ChatGPT plan, signed in.
    Chatgpt,
    /// A model server on this machine or its network: SION_LOCAL_MODEL_URL.
    Local,
    /// A scripted model (`script:<file>`), for demos and tests.
    Script,
}

impl Provider {
    /// The provider of `spec`, `provider:model`.
    pub fn of(spec: &str) -> Result<Provider, CliError> {
        let (kind, model) = spec
            .split_once(':')
            .filter(|(_, model)| !model.trim().is_empty())
            .ok_or_else(|| usage(format!("{spec}: a model is provider:model")))?;
        let _ = model;
        Ok(match kind {
            "anthropic" => Provider::Anthropic,
            "openai-responses" | "openai" => Provider::OpenAi,
            "gemini" => Provider::Gemini,
            "chatgpt" => Provider::Chatgpt,
            "local" => Provider::Local,
            "script" => Provider::Script,
            other => {
                return Err(usage(format!(
                    "{other}: setup knows anthropic, openai-responses, gemini, chatgpt, local and script"
                )))
            }
        })
    }

    /// The variable its key is in, if it needs one.
    pub fn key(self) -> Option<&'static str> {
        match self {
            Provider::Anthropic => Some("ANTHROPIC_API_KEY"),
            Provider::OpenAi => Some("OPENAI_API_KEY"),
            Provider::Gemini => Some("GEMINI_API_KEY"),
            Provider::Chatgpt | Provider::Local | Provider::Script => None,
        }
    }
}

/// How setup asks: at a terminal, unless told to take the defaults.
struct Asker {
    asks: bool,
}

impl Asker {
    fn line(&self, question: &str, default: &str) -> Result<String, CliError> {
        if !self.asks {
            return Ok(default.to_string());
        }
        if default.is_empty() {
            print!("{question}: ");
        } else {
            print!("{question} [{default}]: ");
        }
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        let read = std::io::stdin()
            .lock()
            .read_line(&mut line)
            .map_err(usage)?;
        if read == 0 {
            return Err(usage("setup stopped"));
        }
        let line = line.trim();
        Ok(if line.is_empty() {
            default.to_string()
        } else {
            line.to_string()
        })
    }

    /// A line that may not be empty, asked again until it is not.
    fn named(&self, question: &str) -> Result<String, CliError> {
        loop {
            let answer = self.line(question, "")?;
            if !answer.is_empty() {
                return Ok(answer);
            }
            if !self.asks {
                return Err(usage(format!("{}: give --model", question.trim())));
            }
        }
    }

    fn yes(&self, question: &str, default: bool) -> Result<bool, CliError> {
        if !self.asks {
            return Ok(default);
        }
        loop {
            let answer = self.line(question, if default { "Y/n" } else { "y/N" })?;
            match answer.to_lowercase().as_str() {
                "y/n" => return Ok(default),
                "y" | "yes" => return Ok(true),
                "n" | "no" => return Ok(false),
                _ => println!("  (y or n)"),
            }
        }
    }

    /// One of `options` (numbered from 1), `default` if none is chosen.
    fn choose(
        &self,
        question: &str,
        options: &[String],
        default: usize,
    ) -> Result<usize, CliError> {
        if !self.asks {
            return Ok(default);
        }
        println!("{question}");
        for (i, option) in options.iter().enumerate() {
            println!("  {}. {option}", i + 1);
        }
        loop {
            let answer = self.line("Choose", &(default + 1).to_string())?;
            match answer.parse::<usize>() {
                Ok(n) if (1..=options.len()).contains(&n) => return Ok(n - 1),
                _ => println!("  (a number from 1 to {})", options.len()),
            }
        }
    }
}

/// Runs this `sion` with `args` against the chain at `node`, showing the
/// command and what it printed; returns what it printed.
fn sion(node: &str, args: &[&str]) -> Result<String, CliError> {
    let exe = std::env::current_exe().map_err(usage)?;
    println!("  $ sion {}", args.join(" "));
    let out = Command::new(exe)
        .args(args)
        .env("SION_NODE", node)
        .stdin(Stdio::null())
        .output()
        .map_err(usage)?;
    let printed = String::from_utf8_lossy(&out.stdout).into_owned();
    for line in printed.lines() {
        println!("    {line}");
    }
    if !out.status.success() {
        let said = String::from_utf8_lossy(&out.stderr);
        return Err(usage(format!(
            "sion {} did not work: {}",
            args.join(" "),
            said.trim()
        )));
    }
    Ok(printed)
}

/// Whether the chain at `ledger` answers.
fn answers(ledger: &HttpLedger) -> bool {
    ledger.status().is_ok()
}

/// Waits until the chain answers, for at most `limit`.
fn wait_for(ledger: &HttpLedger, limit: Duration) -> Result<(), CliError> {
    let started = Instant::now();
    let mut told = Instant::now();
    while !answers(ledger) {
        if started.elapsed() > limit {
            return Err(usage(format!(
                "the chain did not answer at {} within {} s",
                ledger.describe(),
                limit.as_secs()
            )));
        }
        if told.elapsed() > Duration::from_secs(15) {
            println!("  (waiting for the chain to answer)");
            told = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Ok(())
}

/// The user's service manager, if one runs for them.
fn user_services() -> Option<Manager> {
    let manager = Manager::here().ok()?;
    match &manager {
        Manager::Launchd { .. } => Some(manager),
        Manager::Systemd { .. } => Command::new("systemctl")
            .args(["--user", "show-environment"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .ok()
            .filter(|status| status.success())
            .map(|_| manager),
    }
}

/// Installs and starts the service `name`, running this `sion` with `args`
/// and `env`, beside `SION_HOME` and the chain's address.
fn service(
    manager: &Manager,
    name: &str,
    node: &str,
    args: &[String],
    env: &[(String, String)],
) -> Result<(), CliError> {
    let program = std::env::current_exe().map_err(usage)?;
    let mut all = vec![
        ("SION_HOME".to_string(), home().display().to_string()),
        ("SION_NODE".to_string(), node.to_string()),
    ];
    all.extend_from_slice(env);
    crate::service::install(manager, name, &program, args, &all)?;
    crate::service::start(manager, name)
}

/// The command line that runs `sion` with `args`, to show.
fn shown(args: &[String]) -> String {
    let words: Vec<String> = args
        .iter()
        .map(|a| {
            if a.chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_./:=@".contains(c))
            {
                a.clone()
            } else {
                format!("'{}'", a.replace('\'', "'\\''"))
            }
        })
        .collect();
    format!("sion {}", words.join(" "))
}

/// Step 1: a chain that answers at `node`.
fn chain(node: &str, args: &SetupArgs, made: &mut Made) -> Result<HttpLedger, CliError> {
    let ledger = HttpLedger::new(node);
    if answers(&ledger) {
        println!("The chain: {node} answers; setup uses it.");
        return Ok(ledger);
    }
    let address = local_address(node).ok_or_else(|| {
        usage(format!(
            "nothing answers at {node}, and setup makes a chain only on this machine \
             (http://127.0.0.1:<port>): start that chain, or unset SION_NODE"
        ))
    })?;
    let data = home().join("chain");
    let run = chain_args(&address, &data, args.idle_round_ms);
    match user_services().filter(|_| !args.no_service) {
        Some(manager) => {
            println!(
                "The chain: one validator of your own, its state in {}, run by your service manager \
                 (service `chain`) so that it starts at each login.",
                data.display()
            );
            service(&manager, "chain", node, &run, &[])?;
            if matches!(manager, Manager::Systemd { .. }) {
                println!(
                    "  (it stops when you log out, unless you let your services linger: loginctl enable-linger)"
                );
            }
            wait_for(&ledger, Duration::from_secs(180))?;
            made.service = true;
        }
        None => {
            println!(
                "The chain: one validator of your own. {}, so run it in a terminal of its own \
                 (setup goes on when it answers):\n\n    {}\n",
                if args.no_service {
                    "You asked for no service"
                } else {
                    "Nothing here runs services for you"
                },
                shown(&run)
            );
            wait_for(&ledger, Duration::from_secs(30 * 60))?;
            made.service = false;
        }
    }
    made.chain = Some(node.to_string());
    println!("  the chain answers at {node}");
    Ok(ledger)
}

/// Test dollars the person's wallet is topped up to, and the agent's.
const PERSON_FUNDS: u64 = 10_000;
const AGENT_FEES: u64 = 100;

/// Step 2: the wallet `name`, made if it is not here, topped up from the
/// faucet if it holds less than a tenth of `funds`.
fn wallet(node: &str, ledger: &HttpLedger, name: &str, funds: u64) -> Result<(), CliError> {
    let seed = crate::session::wallet_dir().join(format!("{name}.seed"));
    if !seed.exists() {
        sion(node, &["keys", "new", name])?;
    }
    let account = crate::session::wallet_for(name).id;
    let held = ledger
        .account(account)
        .map(|view| {
            view.balances
                .iter()
                .filter(|(asset, _)| *asset == AssetId::usd())
                .map(|(_, amount)| *amount)
                .next()
                .unwrap_or(Amount::ZERO)
        })
        .unwrap_or(Amount::ZERO);
    if held.micros() < Amount::units(funds / 10).micros() {
        sion(node, &["faucet", name, &funds.to_string()])?;
    }
    Ok(())
}

/// Step 3: the agent, registered by `person` for `agent_wallet`, unless the
/// one setup made is still on the chain.
fn agent(
    node: &str,
    ledger: &HttpLedger,
    person: &str,
    agent_wallet: &str,
    made: &Made,
) -> Result<ObjectId, CliError> {
    if let Some(agent) = made.agent {
        if made.agent_wallet.as_deref() == Some(agent_wallet)
            && crate::agents::identity(ledger, agent).is_ok()
        {
            println!("The agent: {agent}, registered already.");
            return Ok(agent);
        }
    }
    println!("The agent: registered by {person}, signing as {agent_wallet}.");
    let lifetime = LIFETIME_PULSES.to_string();
    let out = sion(
        node,
        &[
            "agent",
            "register",
            person,
            agent_wallet,
            "--lifetime",
            &lifetime,
        ],
    )?;
    printed_agent(&out).ok_or_else(|| usage("sion agent register printed no agent"))
}

/// Step 4: its memory log, unless the one setup made is still on the chain.
fn memory(
    node: &str,
    ledger: &HttpLedger,
    agent_wallet: &str,
    agent: ObjectId,
    made: &Made,
) -> Result<ObjectId, CliError> {
    if let (Some(log), Some(for_agent)) = (made.memory, made.agent) {
        if for_agent == agent && ledger.object(log).is_ok_and(|o| o.is_some()) {
            println!("Its memory: log {log}, opened already.");
            return Ok(log);
        }
    }
    println!("Its memory: a log of its own.");
    let agent_hex = agent.as_hash().to_hex();
    let out = sion(
        node,
        &["memory", "open", agent_wallet, "--agent", &agent_hex],
    )?;
    printed_log(&out).ok_or_else(|| usage("sion memory open printed no log"))
}

/// The models a server speaking OpenAI's API at `base` serves.
fn served_models(base: &str) -> Result<Vec<String>, CliError> {
    let client = sion_http::Client::new(sion_http::Options {
        allow_private: true,
        timeout: Duration::from_secs(10),
        proxy_from_env: false,
        ..sion_http::Options::default()
    })
    .map_err(usage)?;
    let fetched = client
        .get(&format!("{}/models", base.trim_end_matches('/')), 4 << 20)
        .map_err(|e| usage(format!("{base} did not list its models: {e}")))?;
    let listed: serde_json::Value = serde_json::from_slice(&fetched.body).map_err(usage)?;
    Ok(listed["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| m["id"].as_str().map(str::to_string))
        .collect())
}

/// Keeps the key `variable` for the model: from `from_env`, the
/// environment, what is kept already, or what the person types.
fn key(asker: &Asker, variable: &str, from_env: Option<&str>) -> Result<(), CliError> {
    let store = Credentials::here();
    if let Some(source) = from_env {
        let value = std::env::var(source).map_err(|_| usage(format!("{source} is not set")))?;
        store.keep(variable, &value)?;
        println!(
            "  kept {variable} (from {source}), sealed in {}",
            store.path().display()
        );
        return Ok(());
    }
    if store.get(variable)?.is_some()
        && !asker.yes(&format!("  {variable} is kept already: replace it?"), false)?
    {
        return Ok(());
    }
    if let Some(value) = std::env::var(variable)
        .ok()
        .filter(|v| !v.trim().is_empty())
    {
        if asker.yes(
            &format!("  keep the {variable} set here, so that sion's services have it too?"),
            true,
        )? {
            store.keep(variable, &value)?;
            println!("  kept {variable}, sealed in {}", store.path().display());
        }
        return Ok(());
    }
    if !asker.asks {
        return Err(usage(format!(
            "no {variable}: set it, or give --key-from-env <VARIABLE>, or run setup at a terminal"
        )));
    }
    let value = crate::secrets::read_value(&format!("  {variable}"))?;
    store.keep(variable, &value)?;
    println!("  kept {variable}, sealed in {}", store.path().display());
    Ok(())
}

/// Step 5: the model, and what it needs kept.
fn model(asker: &Asker, args: &SetupArgs, current: Option<&str>) -> Result<String, CliError> {
    let spec = match &args.model {
        Some(spec) => spec.clone(),
        None if !asker.asks => match current {
            Some(spec) => spec.to_string(),
            None => DEFAULT_MODEL.to_string(),
        },
        None => {
            let mut options: Vec<String> = [
                "Claude (Anthropic), with an API key",
                "OpenAI, with an API key",
                "Gemini (Google), with an API key",
                "Your ChatGPT plan, signed in with your browser",
                "A model server on this machine or your network (Ollama, LM Studio, llama.cpp)",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect();
            let mut offset = 0;
            if let Some(spec) = current {
                options.insert(0, format!("Keep {spec}"));
                offset = 1;
            }
            let chosen = asker.choose("Which model does your agent think with?", &options, 0)?;
            if offset == 1 && chosen == 0 {
                current.expect("offered").to_string()
            } else {
                match chosen - offset {
                    0 => format!(
                        "anthropic:{}",
                        asker.line("  Claude model", "claude-sonnet-5-5")?
                    ),
                    1 => format!(
                        "openai-responses:{}",
                        asker.named("  OpenAI model (as OpenAI names it)")?
                    ),
                    2 => format!(
                        "gemini:{}",
                        asker.named("  Gemini model (as Google names it)")?
                    ),
                    3 => {
                        let signed_in = crate::chatgpt::load()
                            .map(|store| store.account.is_some())
                            .unwrap_or(false);
                        if !signed_in {
                            println!("  Sign in with your ChatGPT account:");
                            let exe = std::env::current_exe().map_err(usage)?;
                            let status = Command::new(exe)
                                .args(["agent", "chatgpt", "login"])
                                .status()
                                .map_err(usage)?;
                            if !status.success() {
                                return Err(usage("the ChatGPT sign-in did not finish"));
                            }
                        }
                        let models = crate::chatgpt::models()?;
                        let names: Vec<String> = models
                            .iter()
                            .map(|(slug, shown)| format!("{shown} ({slug})"))
                            .collect();
                        if names.is_empty() {
                            return Err(usage("your plan lists no models"));
                        }
                        let chosen = asker.choose("  Which of your plan's models?", &names, 0)?;
                        format!("chatgpt:{}", models[chosen].0)
                    }
                    _ => {
                        let base = asker.line(
                            "  Its server's address (OpenAI's API)",
                            args.local_url.as_deref().unwrap_or(LOCAL_MODEL_URL),
                        )?;
                        Credentials::here().keep("SION_LOCAL_MODEL_URL", &base)?;
                        let served = served_models(&base)?;
                        if served.is_empty() {
                            return Err(usage(format!(
                                "{base} serves no models: pull one first (ollama pull <model>)"
                            )));
                        }
                        let chosen = asker.choose("  Which of its models?", &served, 0)?;
                        format!("local:{}", served[chosen])
                    }
                }
            }
        }
    };
    let provider = Provider::of(&spec)?;
    if let Some(variable) = provider.key() {
        key(asker, variable, args.key_from_env.as_deref())?;
    }
    if provider == Provider::Local {
        if let Some(url) = &args.local_url {
            Credentials::here().keep("SION_LOCAL_MODEL_URL", url)?;
        }
    }
    if provider == Provider::Chatgpt
        && !crate::chatgpt::load()
            .map(|store| store.account.is_some())
            .unwrap_or(false)
    {
        return Err(usage(
            "sign in with ChatGPT first: sion agent chatgpt login",
        ));
    }
    Ok(spec)
}

/// Checks that the model `spec` answers; returns what it said.
pub fn check_model(spec: &str) -> Result<String, CliError> {
    use sion_agent::model::{Block, Message, Request};
    let model = crate::runtime::model(spec, 0)?;
    let request = Request {
        system: "You are being set up as someone's agent. Answer in a few words.".into(),
        messages: vec![Message::user("Say hello.")],
        tools: Vec::new(),
        max_tokens: 64,
        images: Default::default(),
    };
    let response = model
        .complete(&request)
        .map_err(|e| usage(format!("{spec} did not answer: {e}")))?;
    let said: Vec<&str> = response
        .content
        .iter()
        .filter_map(|block| match block {
            Block::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    Ok(said.join(" ").trim().to_string())
}

/// Step 7: a Telegram bot for `profile`.
fn telegram(asker: &Asker, node: &str, profile: &str, made: &mut Made) -> Result<(), CliError> {
    let token = match std::env::var("TELEGRAM_BOT_TOKEN")
        .ok()
        .filter(|t| !t.trim().is_empty())
    {
        Some(token) => zeroize::Zeroizing::new(token),
        None if asker.asks => {
            println!("  Make a bot with @BotFather on Telegram (/newbot), and paste its token.");
            crate::secrets::read_value("  TELEGRAM_BOT_TOKEN")?
        }
        None => {
            return Err(usage(
                "--telegram needs TELEGRAM_BOT_TOKEN, or a terminal to ask for it",
            ))
        }
    };
    let args: Vec<String> = ["agent", "telegram", profile, "--pairing"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    match user_services() {
        Some(manager) => {
            service(
                &manager,
                "telegram",
                node,
                &args,
                &[("TELEGRAM_BOT_TOKEN".into(), token.to_string())],
            )?;
            made.telegram = true;
            println!(
                "Telegram: a service of yours (`telegram`) answers for {profile}. Write to your bot: \
                 it gives you a pairing code; approve it with sion agent pairing approve <code>."
            );
        }
        None => println!(
            "Telegram: nothing here runs services for you; run the bot in a terminal of its own, with \
             TELEGRAM_BOT_TOKEN set:\n\n    {}\n",
            shown(&args)
        ),
    }
    Ok(())
}

/// Runs `sion setup` against the node at `node`.
pub fn run(node: &str, args: &SetupArgs) -> Result<(), CliError> {
    let asker = Asker {
        asks: !args.yes && std::io::stdin().is_terminal(),
    };
    let home = home();
    std::fs::create_dir_all(&home).map_err(usage)?;
    let mut made = made();
    println!(
        "Setting up Siontology for you in {} (SION_HOME).",
        home.display()
    );
    if !crate::profiles::valid_name(&args.profile) {
        return Err(usage(format!(
            "{:?} is not a profile name: 1 to 32 lowercase letters, digits and hyphens",
            args.profile
        )));
    }

    // 1. The chain.
    let ledger = chain(node, args, &mut made)?;
    save(&made)?;

    // 2. The wallets.
    let login = std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| "me".into());
    let suggested = made.wallet.clone().unwrap_or_else(|| wallet_name(&login));
    let person = match &args.name {
        Some(name) => name.clone(),
        None => asker.line("Your wallet's name", &suggested)?,
    };
    if person != wallet_name(&person) {
        return Err(usage(format!(
            "{person:?}: a wallet's name here is lowercase letters, digits and hyphens"
        )));
    }
    let agent_wallet = format!("{person}-agent");
    println!(
        "Your wallet, {person}, and your agent's, {agent_wallet}: test dollars from the chain's \
         faucet fund its budget and its fees."
    );
    wallet(node, &ledger, &person, PERSON_FUNDS)?;
    wallet(node, &ledger, &agent_wallet, AGENT_FEES)?;
    made.wallet = Some(person.clone());

    // 3, 4. The agent and its memory.
    let agent = agent(node, &ledger, &person, &agent_wallet, &made)?;
    made.agent_wallet = Some(agent_wallet.clone());
    made.agent = Some(agent);
    save(&made)?;
    let log = memory(node, &ledger, &agent_wallet, agent, &made)?;
    made.memory = Some(log);
    save(&made)?;

    // 5. The model.
    let current = crate::profiles::get(&args.profile)
        .ok()
        .and_then(|p| p.models.first().cloned());
    let spec = model(&asker, args, current.as_deref())?;
    if !args.no_check {
        print!("  checking that {spec} answers… ");
        let _ = std::io::stdout().flush();
        match check_model(&spec) {
            Ok(said) => println!("it said: {said}"),
            Err(e) => {
                println!();
                return Err(e);
            }
        }
    }

    // 6. The profile.
    let default_workspace = std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join("sion-workspace"))
        .unwrap_or_else(|| PathBuf::from("sion-workspace"));
    let workspace = match &args.workspace {
        Some(dir) => dir.clone(),
        None => {
            let current = crate::profiles::get(&args.profile)
                .ok()
                .and_then(|p| p.workspace)
                .unwrap_or(default_workspace);
            PathBuf::from(asker.line(
                "The directory it may read and write",
                &current.display().to_string(),
            )?)
        }
    };
    std::fs::create_dir_all(&workspace).map_err(usage)?;
    let workspace = workspace.canonicalize().map_err(usage)?;
    println!("The profile {}: how it runs.", args.profile);
    let agent_hex = agent.as_hash().to_hex();
    let log_hex = log.as_hash().to_hex();
    let workspace_text = workspace.display().to_string();
    sion(
        node,
        &[
            "agent",
            "profile",
            "add",
            &args.profile,
            &agent_wallet,
            &agent_hex,
            "--model",
            &spec,
            "--autonomy",
            &args.autonomy,
            "--memory",
            &log_hex,
            "--workspace",
            &workspace_text,
            "--web",
        ],
    )?;
    made.profile = Some(args.profile.clone());
    save(&made)?;

    // 7. Telegram.
    let wants_telegram =
        args.telegram || (!made.telegram && asker.yes("Answer on Telegram too?", false)?);
    if wants_telegram {
        telegram(&asker, node, &args.profile, &mut made)?;
        save(&made)?;
    }

    println!();
    println!("Ready.");
    println!("  Talk with your agent:           sion");
    println!("  Continue the last conversation: sion -c");
    println!("  Its workspace:                  {}", workspace.display());
    if made.service {
        println!("  The chain's service:            sion agent service list");
    }
    println!("  What this machine has:          sion doctor");
    println!("  A newer release:                sion update");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chain_of_ones_own_is_made_only_on_this_machine() {
        assert_eq!(
            local_address("http://127.0.0.1:8730").as_deref(),
            Some("127.0.0.1:8730")
        );
        assert_eq!(
            local_address("http://localhost:9000/").as_deref(),
            Some("127.0.0.1:9000")
        );
        assert_eq!(
            local_address("http://[::1]:8730").as_deref(),
            Some("[::1]:8730")
        );
        for elsewhere in [
            "http://10.0.0.5:8730",
            "https://127.0.0.1:8730",
            "http://example.com:8730",
            "http://127.0.0.1",
            "http://127.0.0.1:8730/v1",
        ] {
            assert_eq!(local_address(elsewhere), None, "{elsewhere}");
        }
        let args = chain_args("127.0.0.1:8730", Path::new("/home/ana/.sion/chain"), 10_000);
        assert_eq!(
            shown(&args),
            "sion devnet --api 127.0.0.1:8730 --validators 1 --shards 1 --seats 1 \
             --idle-round-ms 10000 --report 0 --data-dir /home/ana/.sion/chain"
        );
        assert_eq!(
            shown(&["a b".into(), "it's".into()]),
            "sion 'a b' 'it'\\''s'"
        );
    }

    #[test]
    fn what_the_commands_print_is_read_back() {
        let registered = "alice signs: Register agent-7 as an agent…\n\
             Pulse 2: Done: agent bdbdb742 registered, signing as agent-7; fee 0.0011 USD.\n\
             agent bdbdb742f506a53671928c83efc1c57321d70dd0636d84ab873575f9f70f60df\n\
             allotment 0f0e…\n";
        assert_eq!(
            printed_agent(registered)
                .map(|a| a.as_hash().to_hex())
                .as_deref(),
            Some("bdbdb742f506a53671928c83efc1c57321d70dd0636d84ab873575f9f70f60df")
        );
        assert_eq!(printed_log(registered), None);
        let opened = "log 6c1f9d2a3b4c5d6e7f8091a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e6\n";
        assert!(printed_log(opened).is_some());
        assert_eq!(printed_agent("agent bdbdb742 registered"), None);
    }

    #[test]
    fn wallet_names_come_from_login_names() {
        assert_eq!(wallet_name("dana"), "dana");
        assert_eq!(wallet_name("Ana.Lima"), "ana-lima");
        assert_eq!(wallet_name("__"), "me");
        assert_eq!(wallet_name(&"x".repeat(40)), "me");
    }

    #[test]
    fn providers_name_what_they_need() {
        assert_eq!(
            Provider::of("anthropic:claude-sonnet-5-5").unwrap().key(),
            Some("ANTHROPIC_API_KEY")
        );
        assert_eq!(
            Provider::of("openai-responses:gpt-5").unwrap(),
            Provider::OpenAi
        );
        assert_eq!(Provider::of("local:llama3.2:3b").unwrap().key(), None);
        assert_eq!(Provider::of("chatgpt:gpt-5").unwrap(), Provider::Chatgpt);
        assert!(Provider::of("anthropic:").is_err());
        assert_eq!(Provider::of("script:x.json").unwrap().key(), None);
        assert!(Provider::of("mistral:large").is_err());
        assert!(Provider::of("claude").is_err());
    }

    #[test]
    fn what_setup_made_round_trips() {
        let made = Made {
            chain: Some("http://127.0.0.1:8730".into()),
            service: true,
            wallet: Some("ana".into()),
            agent_wallet: Some("ana-agent".into()),
            agent: Some(ObjectId::from_hash(Hash32([7; 32]))),
            memory: None,
            profile: Some("assistant".into()),
            telegram: false,
        };
        let text = serde_json::to_string(&made).unwrap();
        assert!(!text.contains("memory"));
        assert_eq!(serde_json::from_str::<Made>(&text).unwrap(), made);
        assert_eq!(serde_json::from_str::<Made>("{}").unwrap(), Made::default());
    }
}
