//! Agent identities (spec v1.8): registering agents, launching subagents
//! with their own identities, cards, suspension, resumption, revocation;
//! cohorts and narrowing (spec v1.14).
//!
//! An agent's identity is a charter with an identity record: the charter's
//! id is the agent's id, its account signs for the agent, and its card
//! commitment says what the agent is. Subagents get their own identity,
//! account, narrower caveats, a budget carved from their parent's allotment
//! and a bucket carved from its bucket.

use clap::Subcommand;
use serde_json::{json, Value};
use sion_assurance::Policy;
use sion_core::hash::Hash32;
use sion_core::ids::{AccountId, AssetId, ObjectId, Pulse};
use sion_core::Amount;
use sion_exec::charter::{
    card_commitment, overlay_id, Caveats, Charter, Narrowing, Spawn, MAX_LINEAGE_DEPTH,
    MAX_SPAWN_BATCH,
};
use sion_exec::cohort::{Materialize, MAX_COHORT, MAX_MATERIALIZE};
use sion_exec::execute::Event;
use sion_exec::object::ObjectData;
use sion_exec::TxKind;

use crate::error::CliError;
use crate::ledger::{Ledger, LedgerExt};
use crate::session::{create_wallet, wallet_dir, Actor, Session};

/// `sion agent …`
#[derive(Debug, Subcommand)]
pub enum AgentCommand {
    /// Register an agent identity: a charter with a card, funded by the
    /// principal.
    Register {
        /// The principal's wallet (who answers for the agent).
        principal: String,
        /// The agent's wallet or account.
        agent: String,
        /// The agent card file (its exact bytes are committed).
        #[arg(long)]
        card: Option<std::path::PathBuf>,
        /// Allowed payees (comma-separated; empty: any).
        #[arg(long, value_delimiter = ',')]
        payees: Vec<String>,
        /// Largest payment (USD).
        #[arg(long, default_value = "100")]
        max_payment: String,
        /// The most it may spend at once: its bucket's capacity, full when
        /// it starts (USD).
        #[arg(long, default_value = "500")]
        burst: String,
        /// What refills its bucket each Pulse (USD).
        #[arg(long, default_value = "0.5")]
        rate: String,
        /// Budget (USD).
        #[arg(long, default_value = "1000")]
        funding: String,
        /// Bond behind the agent's receipts (USD).
        #[arg(long, default_value = "0")]
        bond: String,
        /// Lifetime (Pulses).
        #[arg(long, default_value_t = 100_000)]
        lifetime: u64,
        /// The agent may not launch subagents.
        #[arg(long)]
        no_spawn: bool,
    },
    /// Launch subagents, each with its own identity, account, caveats and
    /// budget carved from the parent's allotment.
    Spawn {
        /// The parent agent's wallet.
        parent: String,
        /// The parent's agent id (its charter).
        agent: String,
        /// The parent's allotment.
        allotment: String,
        /// How many subagents.
        #[arg(long, default_value_t = 1)]
        count: usize,
        /// Budget of each (USD).
        #[arg(long, default_value = "10")]
        budget: String,
        /// Wallet names are `<prefix>-1`, `<prefix>-2`, …; a wallet that
        /// does not exist yet is created with a random seed.
        #[arg(long, default_value = "subagent")]
        prefix: String,
        /// The subagents' card file.
        #[arg(long)]
        card: Option<std::path::PathBuf>,
        /// Largest payment of each (USD); default: the parent's.
        #[arg(long)]
        max_payment: Option<String>,
        /// The bucket capacity of each (USD), taken from what the parent's
        /// bucket holds; default: its budget, or an equal share of what the
        /// parent's bucket holds if that is less.
        #[arg(long)]
        burst: Option<String>,
        /// What refills each one's bucket each Pulse (USD); default: the
        /// share of the parent's refill that its budget is of the parent's
        /// remaining budget.
        #[arg(long)]
        rate: Option<String>,
        /// The subagents may launch subagents of their own.
        #[arg(long)]
        may_spawn: bool,
        /// Keyless subagents (spec v1.13): no keys are made for them. Each
        /// account is derived from the subagent's id, and the wallet that
        /// signs for the parent signs for them, in batches. Their names
        /// become `.keyless` wallet files that hold no secret.
        #[arg(long)]
        keyless: bool,
        /// Give each subagent this much USD from the parent's account for
        /// its transaction fees (all in one transaction).
        #[arg(long)]
        gas: Option<String>,
    },
    /// Pay from several agents in one batch (spec v1.13): one payment from
    /// each agent's own budget, all authorized by one signature of
    /// `signer`, the agents' own wallet or, for keyless agents, the wallet
    /// of their nearest keyed ancestor.
    PayBatch {
        /// The wallet that signs the batch.
        signer: String,
        /// Payee (wallet or account).
        payee: String,
        /// USD each agent pays.
        amount: String,
        /// The agents (their ids, comma-separated), at most 256.
        #[arg(long, value_delimiter = ',', required = true)]
        agents: Vec<String>,
        /// What the payments are for.
        #[arg(long, default_value = "")]
        purpose: String,
        /// Print the batch as `POST /v1/batches` takes it (Borsh, in hex)
        /// instead of sending it.
        #[arg(long)]
        print: bool,
    },
    /// Show an agent: account, principal, lineage, card, status, budget.
    Show {
        /// Agent id.
        agent: String,
    },
    /// Replace an agent's card with a file's commitment.
    Card {
        /// The agent's wallet.
        wallet: String,
        /// Agent id.
        agent: String,
        /// The card file.
        file: std::path::PathBuf,
    },
    /// Print an agent card template (A2A-style JSON with a `sion` section).
    CardTemplate {
        /// The agent's name.
        name: String,
        /// What it does.
        #[arg(long, default_value = "")]
        description: String,
        /// Where it is served.
        #[arg(long, default_value = "")]
        url: String,
    },
    /// Suspend an agent and everything below it (an overseer in good
    /// standing: the backer, or the agent of a charter above it that is
    /// neither revoked nor suspended). The highest suspender is recorded.
    Suspend {
        /// The overseer's wallet.
        wallet: String,
        /// Agent id.
        agent: String,
    },
    /// Lift a suspension (the recorded suspender, in good standing, or the
    /// backer).
    Resume {
        /// Wallet.
        wallet: String,
        /// Agent id.
        agent: String,
    },
    /// Revoke an agent and everything below it, for good (the agent itself
    /// or an overseer in good standing).
    Revoke {
        /// Wallet.
        wallet: String,
        /// Agent id.
        agent: String,
    },
    /// Return a revoked or expired agent's unspent budget to its backer
    /// (its principal or its backer).
    Reclaim {
        /// Wallet.
        wallet: String,
        /// Agent id.
        agent: String,
    },
    /// Add limits to an agent and everything below it (spec v1.14): its
    /// payees, its largest payment, its last Pulse, whether it may spawn.
    /// What is not given stays as it is. Only the backer may widen or lift
    /// a narrowing.
    Narrow {
        /// The wallet (the agent, an overseer or the backer).
        wallet: String,
        /// Agent id.
        agent: String,
        /// Allowed payees (comma-separated).
        #[arg(long, value_delimiter = ',')]
        payees: Vec<String>,
        /// Largest payment (USD).
        #[arg(long)]
        max_payment: Option<String>,
        /// The subtree may pay for this many more Pulses.
        #[arg(long)]
        expires_in: Option<u64>,
        /// The subtree may no longer launch subagents.
        #[arg(long)]
        no_spawn: bool,
        /// Lift the narrowing (the backer only).
        #[arg(long, conflicts_with_all = ["payees", "max_payment", "expires_in", "no_spawn"])]
        lift: bool,
    },
    /// Commit to up to 1,048,576 keyless subagents in one transaction (spec
    /// v1.14): a cohort, the root of a sum tree over their buckets. A
    /// member costs nothing on the ledger until `agent materialize` creates
    /// it, with its budget from the parent's allotment then. The cohort is
    /// kept in $SION_HOME/agents/cohorts, to prove its members later.
    Cohort {
        /// The parent agent's wallet.
        parent: String,
        /// The parent's agent id.
        agent: String,
        /// The parent's allotment.
        allotment: String,
        /// How many members.
        #[arg(long)]
        count: u32,
        /// Budget of each, paid when it is materialized (USD).
        #[arg(long, default_value = "1")]
        budget: String,
        /// Bucket capacity of each (USD).
        #[arg(long, default_value = "1")]
        burst: String,
        /// What refills each one's bucket each Pulse (USD).
        #[arg(long, default_value = "0")]
        rate: String,
        /// The members' buckets start full, their tokens taken from the
        /// parent's now; otherwise they start empty and fill at their rate
        /// from the cohort's creation.
        #[arg(long)]
        full: bool,
        /// The members' card file.
        #[arg(long)]
        card: Option<std::path::PathBuf>,
        /// Largest payment of each (USD); default: the parent's.
        #[arg(long)]
        max_payment: Option<String>,
        /// Wallet names of materialized members are `<prefix>-<index>`.
        #[arg(long, default_value = "member")]
        prefix: String,
    },
    /// Materialize members of a cohort: their charters and allotments, at
    /// their lineage ids, with proofs of their leaves. The cohort parent's
    /// wallet signs, and signs for them afterwards.
    Materialize {
        /// The cohort parent's wallet.
        parent: String,
        /// The cohort (its id, or a unique prefix of it).
        cohort: String,
        /// The members' indices (comma-separated); default: the next
        /// `--next` not materialized yet.
        #[arg(long, value_delimiter = ',')]
        index: Vec<u32>,
        /// How many of the next members not materialized yet.
        #[arg(long, default_value_t = 1)]
        next: u32,
    },
    /// Put an agent to work on a task: a model drives it, with the ledger
    /// (balance, payments under its charter, transactions), optionally
    /// files in a workspace and the web, as tools. The session is kept, so
    /// one waiting for your approval can resume.
    Run {
        /// The agent's wallet.
        wallet: String,
        /// The agent (its id).
        agent: String,
        /// The task, in words.
        #[arg(long)]
        task: String,
        /// The model: anthropic:<model> (ANTHROPIC_API_KEY), openai:<model>
        /// (OPENAI_BASE_URL, OPENAI_API_KEY) or script:<file>. Repeat for
        /// fallbacks, in order.
        #[arg(long = "model", required = true)]
        models: Vec<String>,
        /// How much it may do without asking: L0 (nothing) to L6 (spend
        /// within its charter). L3 reads and writes its workspace; L5 also
        /// fetches from the web and asks before paying.
        #[arg(long, default_value = "L5")]
        autonomy: String,
        /// A directory the agent may read and write.
        #[arg(long)]
        workspace: Option<std::path::PathBuf>,
        /// Let it fetch public web pages (they taint the session: external
        /// calls and payments then wait for approval).
        #[arg(long)]
        web: bool,
        /// A file with the system prompt (default: one that tells the agent
        /// who it is and how its charter limits it).
        #[arg(long)]
        system: Option<std::path::PathBuf>,
        /// The agent's memory log (its id, or a unique prefix of one with a
        /// local store), or `new` to open one for it. Its preferences, goals
        /// and procedures go into the prompt; what it records is committed
        /// when the session ends, with the session as an episode.
        #[arg(long)]
        memory: Option<String>,
        /// The directory of the agent's skills (default:
        /// $SION_HOME/agents/skills/<agent>).
        #[arg(long)]
        skills: Option<std::path::PathBuf>,
        /// Do not ask the agent, after a long task it finished without
        /// writing a skill, whether to write down how it did it.
        #[arg(long)]
        no_reflect: bool,
        /// Let it hand work to other agents: spawn subagents with budgets
        /// carved from its own (agent_spawn), hire through Task Charters
        /// (task_post, task_status) and deliver tasks it was hired for
        /// (task_submit).
        #[arg(long)]
        delegate: bool,
        /// The model its subagents run with (default: its own). Repeat for
        /// fallbacks.
        #[arg(long = "subagent-model")]
        subagent_models: Vec<String>,
        /// The adjudicator atom of the tasks it posts (without one, their
        /// results cannot be challenged).
        #[arg(long)]
        adjudicator: Option<String>,
        #[command(flatten)]
        shell: crate::runtime::ShellArgs,
        /// Give it the reviewed tools of this MCP server (see `sion agent mcp
        /// add`). Repeat for more.
        #[arg(long = "mcp", value_name = "SERVER")]
        mcp: Vec<String>,
        /// Give it the storage-deal tools: keep a file of its workspace with
        /// storage providers (the escrow comes from its account's balance,
        /// not its budget), follow its deals and close them.
        #[arg(long)]
        storage: bool,
    },
    /// Continue a session: approve or decline the call it waits on, or tell
    /// it more.
    Continue {
        /// The session (its id, or a unique prefix of it).
        session: String,
        /// Approve the call it waits on.
        #[arg(long, conflicts_with_all = ["decline", "say"])]
        approve: bool,
        /// Decline the call it waits on, saying why.
        #[arg(long, conflicts_with = "say")]
        decline: Option<String>,
        /// Tell it something more and let it go on.
        #[arg(long)]
        say: Option<String>,
    },
    /// The agent sessions on this machine.
    Sessions,
    /// A session as one JSON line in chat format with tool calls, with its
    /// outcome: for review, evaluation or training.
    Trace {
        /// The session (its id, or a unique prefix of it).
        session: String,
    },
    /// The kill switch: `on` stops every agent run on this machine before
    /// its next call; `off` lets them run again.
    KillSwitch {
        /// on or off.
        state: String,
    },
    /// Agent profiles: how to run an agent, under a name, for schedules
    /// and channels.
    #[command(subcommand)]
    Profile(crate::profiles::ProfileCommand),
    /// MCP servers whose tools agents may use: add one (its tools are
    /// reviewed and pinned), list them, review one again, remove one.
    #[command(subcommand)]
    Mcp(crate::agent_mcp::McpCommand),
    /// Scheduled jobs: a task a profile works on at a time, every interval
    /// or on a cron expression, each run a session whose answer goes to a
    /// channel.
    #[command(subcommand)]
    Schedule(crate::schedules::ScheduleCommand),
    /// Run scheduled jobs as they fall due.
    Scheduler {
        /// Run the jobs due now, then stop.
        #[arg(long)]
        once: bool,
        /// Check for new jobs at least this often, in seconds.
        #[arg(long, default_value_t = 60)]
        poll: u64,
    },
    /// Serve the HTTP channel: POST /v1/profiles/<name>/messages with
    /// {"text": …, "conversation": …}, and GET /v1/sessions/<id>, with a
    /// bearer token.
    Serve {
        /// Where to listen.
        #[arg(long, default_value = "127.0.0.1:8790")]
        listen: String,
        /// The environment variable holding the bearer token (at least 16
        /// characters).
        #[arg(long, default_value = "SION_AGENT_TOKEN")]
        token_env: String,
    },
    /// Answer Telegram chats with a profile, by long polling
    /// (TELEGRAM_BOT_TOKEN; TELEGRAM_API_BASE for another server).
    Telegram {
        /// The agent profile.
        profile: String,
        /// The chats it answers (comma-separated ids); others are ignored.
        #[arg(long, value_delimiter = ',', required = true)]
        allow: Vec<i64>,
        /// Answer one batch of messages, then stop.
        #[arg(long)]
        once: bool,
    },
}

/// Runs an agent command. `shared` is the same ledger, for the runtime's
/// tools, which keep it between calls.
pub fn run(
    session: &mut Session<'_>,
    ledger: &dyn Ledger,
    shared: crate::runtime::SharedLedger,
    command: AgentCommand,
) -> Result<(), CliError> {
    match command {
        AgentCommand::Run {
            wallet,
            agent,
            task,
            models,
            autonomy,
            workspace,
            web,
            system,
            memory,
            skills,
            no_reflect,
            delegate,
            subagent_models,
            adjudicator,
            shell,
            mcp,
            storage,
        } => {
            let agent = parse_id(&agent, "agent")?;
            let shell = shell.sandbox()?;
            let adjudicator = adjudicator
                .map(|a| parse_id(&a, "adjudicator").map(|id| sion_core::ids::AtomId(id.0)))
                .transpose()?;
            let memory = match memory.as_deref() {
                None => None,
                Some("new") => {
                    let (view, opened) = crate::memory::open(session, &wallet, Some(agent))?;
                    print_receipt(session, &view);
                    let log = opened
                        .ok_or_else(|| CliError::Transaction("no memory log was opened".into()))?;
                    println!("memory log {log}");
                    Some(log)
                }
                Some(text) => Some(crate::memory::find_log(ledger, text)?),
            };
            let policy = sion_agent::Policy {
                autonomy: autonomy.parse().map_err(CliError::Usage)?,
                ..sion_agent::Policy::default()
            };
            let system = system
                .map(|path| {
                    std::fs::read_to_string(&path)
                        .map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))
                })
                .transpose()?;
            let (saved, lines) = crate::runtime::start(
                shared,
                &wallet,
                agent,
                &task,
                models,
                policy,
                crate::runtime::Equipment {
                    workspace,
                    web,
                    memory,
                    skills,
                    system,
                    reflect: !no_reflect,
                    delegate,
                    subagent_models,
                    adjudicator,
                    depth: 0,
                    shell,
                    mcp,
                    storage,
                },
            )?;
            println!("session {} for agent {}", saved.state.id, agent.short());
            for line in lines {
                println!("{line}");
            }
            Ok(())
        }
        AgentCommand::Continue {
            session: id,
            approve,
            decline,
            say,
        } => {
            let reply = if approve {
                Some(crate::runtime::Reply::Approve)
            } else if let Some(why) = decline {
                Some(crate::runtime::Reply::Decline(why))
            } else {
                say.map(crate::runtime::Reply::Say)
            };
            let (_, lines) = crate::runtime::resume(shared, &id, reply)?;
            for line in lines {
                println!("{line}");
            }
            Ok(())
        }
        AgentCommand::Sessions => {
            let dir = crate::runtime::sessions_dir();
            let mut rows = Vec::new();
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if let Some(id) = name.strip_suffix(".json") {
                        if let Ok(saved) = crate::runtime::Saved::load(id) {
                            let task = saved
                                .state
                                .messages
                                .first()
                                .map(|m| m.text())
                                .unwrap_or_default();
                            let ended = match &saved.ended {
                                Some(sion_agent::Ended::Done { .. }) => "done".to_string(),
                                Some(sion_agent::Ended::NeedsApproval { tool, .. }) => {
                                    format!("waiting on {tool}")
                                }
                                Some(other) => format!("{other:?}").to_lowercase(),
                                None => "new".into(),
                            };
                            rows.push(format!(
                                "{id}  {}  {ended}  {}",
                                saved.wallet,
                                task.chars().take(60).collect::<String>()
                            ));
                        }
                    }
                }
            }
            rows.sort();
            if rows.is_empty() {
                println!("no agent sessions in {}", dir.display());
            }
            for row in rows {
                println!("{row}");
            }
            Ok(())
        }
        AgentCommand::Trace { session: id } => {
            let saved = crate::runtime::Saved::load(&id)?;
            println!(
                "{}",
                sion_agent::trace::chat_line(&saved.state, &saved.prompt, saved.ended.as_ref())
            );
            Ok(())
        }
        AgentCommand::Profile(command) => crate::profiles::run(ledger, command),
        AgentCommand::Mcp(command) => crate::agent_mcp::run(command),
        AgentCommand::Schedule(command) => crate::schedules::run(command),
        AgentCommand::Scheduler { once, poll } => crate::schedules::scheduler(shared, once, poll),
        AgentCommand::Serve { listen, token_env } => {
            let token = std::env::var(&token_env)
                .map_err(|_| CliError::Usage(format!("set {token_env} to the channel's token")))?;
            crate::channels::serve(shared, &listen, &token, None)
        }
        AgentCommand::Telegram {
            profile,
            allow,
            once,
        } => {
            let profile = crate::profiles::get(&profile)?;
            crate::channels::telegram(shared, &profile, &allow, once)
        }
        AgentCommand::KillSwitch { state } => {
            let path = crate::runtime::kill_switch();
            match state.as_str() {
                "on" => {
                    if let Some(dir) = path.parent() {
                        std::fs::create_dir_all(dir).map_err(|e| CliError::Usage(e.to_string()))?;
                    }
                    std::fs::write(&path, "agents stop before their next call\n")
                        .map_err(|e| CliError::Usage(e.to_string()))?;
                    println!(
                        "kill switch on: every agent run stops before its next call ({})",
                        path.display()
                    );
                }
                "off" => {
                    let _ = std::fs::remove_file(&path);
                    println!("kill switch off");
                }
                other => {
                    return Err(CliError::Usage(format!(
                        "the kill switch is on or off, not {other}"
                    )))
                }
            }
            Ok(())
        }
        AgentCommand::Register {
            principal,
            agent,
            card,
            payees,
            max_payment,
            burst,
            rate,
            funding,
            bond,
            lifetime,
            no_spawn,
        } => {
            let agent = session.id(&agent);
            let payees = if payees.is_empty() {
                None
            } else {
                Some(payees.iter().map(|p| session.id(p)).collect())
            };
            let caveats = Caveats {
                payees,
                max_per_payment: parse_amount(&max_payment)?,
                burst: parse_amount(&burst)?,
                rate: parse_amount(&rate)?,
                expires_at: Pulse(ledger.pulse()?.0 + lifetime),
                evidence_above: None,
                evidence_policy: Policy::standard(),
                receipts: true,
                may_delegate: !no_spawn,
            };
            let card = read_card(card.as_deref())?;
            let view = session.send(
                &principal,
                TxKind::RegisterAgent {
                    agent,
                    card,
                    caveats,
                    funding: parse_amount(&funding)?,
                    agent_bond: parse_amount(&bond)?,
                },
                Vec::new(),
            )?;
            print_receipt(session, &view);
            for event in &view.effects.events {
                if let Event::AgentRegistered {
                    agent, allotment, ..
                } = event
                {
                    println!(
                        "agent {agent}\nallotment {allotment}\ndid {}",
                        did(ledger, agent)?
                    );
                }
            }
            Ok(())
        }
        AgentCommand::Spawn {
            parent,
            agent,
            allotment,
            count,
            budget,
            prefix,
            card,
            max_payment,
            burst,
            rate,
            may_spawn,
            keyless,
            gas,
        } => {
            if count == 0 {
                return Err(CliError::Usage("--count must be at least 1".into()));
            }
            let charter = parse_id(&agent, "agent")?;
            let allotment = parse_id(&allotment, "allotment")?;
            let own = identity(ledger, charter)?;
            let mut caveats = own.caveats.clone();
            if let Some(max) = max_payment {
                caveats.max_per_payment = parse_amount(&max)?;
            }
            let budget = parse_amount(&budget)?;
            // Buckets are carved from the parent's (spec v1.14): by default
            // each child may spend its budget at once, or an equal share of
            // what the parent's bucket holds if that is less, and gets the
            // share of the parent's refill that its budget is of the
            // parent's remaining budget.
            let (kept, remaining) = match ledger.object(allotment)?.map(|o| o.data) {
                Some(ObjectData::Allotment(a)) => (a.bucket, a.remaining),
                _ => return Err(CliError::Usage(format!("{allotment} is not an allotment"))),
            };
            let holds = kept.level(ledger.pulse()?).min(kept.burst);
            caveats.burst = match burst {
                Some(burst) => parse_amount(&burst)?,
                None => Amount(budget.0.min(holds.0 / count as u128)),
            };
            caveats.rate = match rate {
                Some(rate) => parse_amount(&rate)?,
                None => Amount(kept.rate.0.saturating_mul(budget.0) / remaining.0.max(1)),
            };
            caveats.may_delegate = may_spawn && own.caveats.may_delegate;
            let card = read_card(card.as_deref())?;
            let gas = gas.as_deref().map(parse_amount).transpose()?;
            let signer = session.signer_of(&parent);
            let names: Vec<String> = (1..=count).map(|i| format!("{prefix}-{i}")).collect();
            for name in &names {
                if keyless {
                    let dir = crate::session::wallet_dir();
                    for file in [format!("{name}.seed"), format!("{name}.keyless")] {
                        if dir.join(&file).exists() {
                            return Err(CliError::Usage(format!(
                                "{} already exists: choose another --prefix",
                                dir.join(file).display()
                            )));
                        }
                    }
                } else {
                    ensure_wallet(name)?;
                }
            }
            let mut accounts = Vec::with_capacity(count);
            for batch in names.chunks(MAX_SPAWN_BATCH) {
                let children: Vec<Spawn> = batch
                    .iter()
                    .map(|name| Spawn {
                        agent: (!keyless).then(|| session.id(name)),
                        card,
                        caveats: caveats.clone(),
                        budget,
                    })
                    .collect();
                let input = session.reference(&allotment)?;
                let view = session.send(
                    &parent,
                    TxKind::SpawnAgents { charter, children },
                    vec![input],
                )?;
                print_receipt(session, &view);
                let spawned: Vec<(ObjectId, AccountId, ObjectId)> = view
                    .effects
                    .events
                    .iter()
                    .filter_map(|event| match event {
                        Event::AgentSpawned {
                            agent,
                            account,
                            allotment,
                            ..
                        } => Some((*agent, *account, *allotment)),
                        _ => None,
                    })
                    .collect();
                for (name, (id, account, own_allotment)) in batch.iter().zip(&spawned) {
                    if keyless {
                        crate::session::create_keyless_alias(
                            name,
                            &crate::session::KeylessAlias {
                                charter: *id,
                                signer: signer.clone(),
                            },
                        )?;
                        println!(
                            "{name}: agent {id} account {account} allotment {own_allotment} (keyless; {signer} signs for it)"
                        );
                    } else {
                        println!("{name}: agent {id} allotment {own_allotment}");
                    }
                    accounts.push(*account);
                }
            }
            if let Some(gas) = gas {
                for chunk in accounts.chunks(256) {
                    let view = session.send(
                        &parent,
                        TxKind::Transfer {
                            asset: AssetId::usd(),
                            outputs: chunk.iter().map(|account| (*account, gas)).collect(),
                        },
                        Vec::new(),
                    )?;
                    print_receipt(session, &view);
                }
            }
            Ok(())
        }
        AgentCommand::PayBatch {
            signer,
            payee,
            amount,
            agents,
            purpose,
            print,
        } => {
            if agents.len() > sion_exec::tx::MAX_BATCH as usize {
                return Err(CliError::Usage(format!(
                    "a batch holds at most {} transactions",
                    sion_exec::tx::MAX_BATCH
                )));
            }
            let amount = parse_amount(&amount)?;
            let payee = session.id(&payee);
            let mut actions = Vec::with_capacity(agents.len());
            for agent in &agents {
                let charter = parse_id(agent, "agent")?;
                let state = identity(ledger, charter)?;
                // A keyless agent's own account sends; a keyed one is the
                // signer's own.
                let actor = if sion_exec::charter::is_keyless(&charter, &state) {
                    Actor::Keyless(charter)
                } else {
                    Actor::Wallet(signer.clone())
                };
                let mut source = None;
                for id in &state.allotments {
                    if let Some(object) = ledger.object(*id)? {
                        if matches!(&object.data, ObjectData::Allotment(a) if a.remaining >= amount)
                        {
                            source = Some(object.reference());
                            break;
                        }
                    }
                }
                let source = source.ok_or_else(|| {
                    CliError::Usage(format!(
                        "agent {} has no allotment holding {amount} USD",
                        charter.short()
                    ))
                })?;
                actions.push((
                    actor,
                    TxKind::AgentPay {
                        charter,
                        payee,
                        amount,
                        purpose: purpose.clone(),
                        evidence: None,
                    },
                    vec![source],
                ));
            }
            if print {
                let members = session.sign_batch(&signer, actions)?;
                session.settle_all();
                let envelope = sion_exec::tx::BatchEnvelope::of(&members)
                    .ok_or_else(|| CliError::Protocol("not one whole batch".into()))?;
                println!("{}", hex::encode(sion_core::encode::encode(&envelope)));
                return Ok(());
            }
            let views = session.send_batch(&signer, actions)?;
            for view in &views {
                print_receipt(session, view);
            }
            Ok(())
        }
        AgentCommand::Show { agent } => {
            let id = parse_id(&agent, "agent")?;
            println!(
                "{}",
                serde_json::to_string_pretty(&describe(ledger, id)?).unwrap_or_default()
            );
            Ok(())
        }
        AgentCommand::Card {
            wallet,
            agent,
            file,
        } => {
            let card = read_card(Some(&file))?;
            let view = session.send(
                &wallet,
                TxKind::SetAgentCard {
                    charter: parse_id(&agent, "agent")?,
                    card,
                },
                Vec::new(),
            )?;
            print_receipt(session, &view);
            Ok(())
        }
        AgentCommand::CardTemplate {
            name,
            description,
            url,
        } => {
            println!(
                "{}",
                serde_json::to_string_pretty(&card_template(&name, &description, &url))
                    .unwrap_or_default()
            );
            Ok(())
        }
        AgentCommand::Suspend { wallet, agent } => {
            let charter = parse_id(&agent, "agent")?;
            let view = session.send(&wallet, TxKind::SuspendAgent { charter }, Vec::new())?;
            print_receipt(session, &view);
            Ok(())
        }
        AgentCommand::Resume { wallet, agent } => {
            let charter = parse_id(&agent, "agent")?;
            let view = session.send(&wallet, TxKind::ResumeAgent { charter }, Vec::new())?;
            print_receipt(session, &view);
            Ok(())
        }
        AgentCommand::Revoke { wallet, agent } => {
            let charter = parse_id(&agent, "agent")?;
            let view = session.send(&wallet, TxKind::RevokeCharter { charter }, Vec::new())?;
            print_receipt(session, &view);
            Ok(())
        }
        AgentCommand::Reclaim { wallet, agent } => {
            let charter = parse_id(&agent, "agent")?;
            let view = session.send(&wallet, TxKind::ReclaimCharter { charter }, Vec::new())?;
            print_receipt(session, &view);
            Ok(())
        }
        AgentCommand::Narrow {
            wallet,
            agent,
            payees,
            max_payment,
            expires_in,
            no_spawn,
            lift,
        } => {
            let charter = parse_id(&agent, "agent")?;
            let narrowing = if lift {
                None
            } else {
                let state = identity(ledger, charter)?;
                // What is not given stays as the narrowing it has, or else
                // as its own caveats.
                let mut n = overlay(ledger, &charter)?
                    .and_then(|o| o.narrowing)
                    .unwrap_or(Narrowing {
                        payees: state.caveats.payees.clone(),
                        max_per_payment: state.caveats.max_per_payment,
                        expires_at: state.caveats.expires_at,
                        may_delegate: state.caveats.may_delegate,
                    });
                if !payees.is_empty() {
                    n.payees = Some(payees.iter().map(|p| session.id(p)).collect());
                }
                if let Some(max) = max_payment {
                    n.max_per_payment = parse_amount(&max)?;
                }
                if let Some(pulses) = expires_in {
                    n.expires_at = Pulse(ledger.pulse()?.0.saturating_add(pulses));
                }
                if no_spawn {
                    n.may_delegate = false;
                }
                Some(n)
            };
            let view = session.send(
                &wallet,
                TxKind::NarrowAgent { charter, narrowing },
                Vec::new(),
            )?;
            print_receipt(session, &view);
            Ok(())
        }
        AgentCommand::Cohort {
            parent,
            agent,
            allotment,
            count,
            budget,
            burst,
            rate,
            full,
            card,
            max_payment,
            prefix,
        } => {
            if count == 0 || count > MAX_COHORT {
                return Err(CliError::Usage(format!(
                    "--count must be between 1 and {MAX_COHORT}"
                )));
            }
            let charter = parse_id(&agent, "agent")?;
            let allotment = parse_id(&allotment, "allotment")?;
            let own = identity(ledger, charter)?;
            let mut plan = crate::cohorts::Plan {
                cohort: None,
                parent: charter,
                first: 0,
                count,
                budget: parse_amount(&budget)?,
                burst: parse_amount(&burst)?,
                rate: parse_amount(&rate)?,
                card: read_card(card.as_deref())?,
                prefix,
                signer: session.signer_of(&parent),
            };
            let mut caveats = own.caveats.clone();
            if let Some(max) = max_payment {
                caveats.max_per_payment = parse_amount(&max)?;
            }
            caveats.burst = plan.burst;
            caveats.rate = plan.rate;
            caveats.may_delegate = false;
            let started = std::time::Instant::now();
            let tree = plan.tree()?;
            let root = tree.root();
            println!(
                "sum tree over {count} members built in {} ms: root {}, {} of burst and {} a Pulse of refill in all",
                started.elapsed().as_millis(),
                root.hash.short(),
                root.burst,
                root.rate
            );
            let input = session.reference(&allotment)?;
            let view = session.send(
                &parent,
                TxKind::SpawnCohort {
                    charter,
                    root: root.hash,
                    count,
                    burst: root.burst,
                    rate: root.rate,
                    full,
                    caveats,
                },
                vec![input],
            )?;
            print_receipt(session, &view);
            for event in &view.effects.events {
                if let Event::CohortSpawned { cohort, first, .. } = event {
                    plan.cohort = Some(*cohort);
                    plan.first = *first;
                    let path = crate::cohorts::save(&plan)?;
                    println!(
                        "cohort {cohort}\nmembers: lineage indices {first} to {} under agent {}, kept in {}",
                        first + u64::from(count) - 1,
                        charter.short(),
                        path.display()
                    );
                }
            }
            Ok(())
        }
        AgentCommand::Materialize {
            parent,
            cohort,
            index,
            next,
        } => {
            let plan = crate::cohorts::find(&cohort)?;
            let indices: Vec<u32> = if index.is_empty() {
                let mut found = Vec::new();
                for i in 0..plan.count {
                    if found.len() as u32 >= next {
                        break;
                    }
                    if ledger.object(plan.member_id(i))?.is_none() {
                        found.push(i);
                    }
                }
                found
            } else {
                let mut sorted = index;
                sorted.sort_unstable();
                sorted.dedup();
                sorted
            };
            if indices.is_empty() {
                return Err(CliError::Usage("no member left to materialize".into()));
            }
            let tree = plan.tree()?;
            let state = identity(ledger, plan.parent)?;
            let allotment =
                state.allotments.iter().next().copied().ok_or_else(|| {
                    CliError::Usage("the cohort's parent has no allotment".into())
                })?;
            for chunk in indices.chunks(MAX_MATERIALIZE) {
                let members = chunk
                    .iter()
                    .map(|i| {
                        tree.prove(*i)
                            .map(|siblings| Materialize {
                                index: *i,
                                member: plan.member(),
                                siblings,
                            })
                            .ok_or_else(|| CliError::Usage(format!("the cohort has no member {i}")))
                    })
                    .collect::<Result<Vec<_>, CliError>>()?;
                let input = session.reference(&allotment)?;
                let view = session.send(
                    &parent,
                    TxKind::MaterializeAgents {
                        cohort: plan.cohort_id()?,
                        members,
                    },
                    vec![input],
                )?;
                print_receipt(session, &view);
                for event in &view.effects.events {
                    if let Event::AgentMaterialized {
                        agent,
                        index,
                        account,
                        allotment,
                        ..
                    } = event
                    {
                        let name = format!("{}-{index}", plan.prefix);
                        crate::session::create_keyless_alias(
                            &name,
                            &crate::session::KeylessAlias {
                                charter: *agent,
                                signer: plan.signer.clone(),
                            },
                        )?;
                        println!(
                            "{name}: agent {agent} account {account} allotment {allotment} (keyless; {} signs for it)",
                            plan.signer
                        );
                    }
                }
            }
            Ok(())
        }
    }
}

/// An agent card template: the fields of an A2A agent card, plus a `sion`
/// section that ties the card to its identity once registered. The card's
/// commitment is [`card_commitment`] of the file's exact bytes.
pub fn card_template(name: &str, description: &str, url: &str) -> Value {
    json!({
        "name": name,
        "description": description,
        "url": url,
        "version": "1.0.0",
        "provider": { "organization": "", "url": "" },
        "capabilities": { "streaming": false, "pushNotifications": false },
        "defaultInputModes": ["text/plain"],
        "defaultOutputModes": ["text/plain"],
        "skills": [],
        "sion": {
            "agent": "did:sion:<chain identity>:<agent id, filled in after registration>",
            "account": "<the account that signs for the agent>",
            "principal": "<the account that answers for it>",
            "parent": null
        }
    })
}

/// The overlay of `agent`: what happened to it after it was created, if
/// anything did (spec v1.14).
pub fn overlay(
    ledger: &dyn Ledger,
    agent: &ObjectId,
) -> Result<Option<sion_exec::charter::AgentOverlay>, CliError> {
    match ledger.object(overlay_id(agent))?.map(|o| o.data) {
        Some(ObjectData::AgentOverlay(o)) => Ok(Some(o)),
        _ => Ok(None),
    }
}

/// An agent's identity record, lineage, overlays and status, as JSON.
pub fn describe(ledger: &dyn Ledger, id: ObjectId) -> Result<Value, CliError> {
    let charter = identity(ledger, id)?;
    let keyless = sion_exec::charter::is_keyless(&id, &charter);
    let own = overlay(ledger, &id)?;
    // The ancestors' ids come from the lineage (spec v1.14), nearest first.
    // A keyless agent's signer is the agent of its nearest keyed ancestor
    // (spec §4.3).
    let mut lineage = Vec::new();
    let mut signer = None;
    let mut stopped = false;
    let mut narrowed = own.as_ref().is_some_and(|o| o.narrowing.is_some());
    for ancestor in charter.lineage.ancestry().iter().rev().skip(1) {
        if lineage.len() == MAX_LINEAGE_DEPTH {
            break;
        }
        let above = identity(ledger, *ancestor)?;
        if keyless && signer.is_none() && !sion_exec::charter::is_keyless(ancestor, &above) {
            signer = Some(above.agent);
        }
        let o = overlay(ledger, ancestor)?;
        stopped |= o
            .as_ref()
            .is_some_and(|o| o.revoked_at.is_some() || o.suspended_by.is_some());
        narrowed |= o.as_ref().is_some_and(|o| o.narrowing.is_some());
        lineage.push(json!({
            "agent": ancestor,
            "account": above.agent,
            "suspended_by": o.as_ref().and_then(|o| o.suspended_by),
            "revoked_at": o.as_ref().and_then(|o| o.revoked_at),
            "narrowing": o.as_ref().and_then(|o| o.narrowing.as_ref()).map(narrowing_json),
        }));
    }
    // A payment sent now executes in the next Pulse at the earliest.
    let next = ledger.pulse()?.next();
    let (mut remaining, mut now, mut burst, mut rate) = (0u128, 0u128, 0u128, 0u128);
    for allotment in &charter.allotments {
        if let Some(object) = ledger.object(*allotment)? {
            if let ObjectData::Allotment(a) = object.data {
                remaining += a.remaining.0;
                now += a.bucket.level(next).min(a.remaining).0;
                burst += a.bucket.burst.0;
                rate += a.bucket.rate.0;
            }
        }
    }
    let narrowings_end = lineage
        .iter()
        .filter_map(|entry| entry["narrowing"]["expires_at"].as_u64())
        .chain(
            own.as_ref()
                .and_then(|o| o.narrowing.as_ref())
                .map(|n| n.expires_at.0),
        )
        .min();
    let expired =
        next > charter.caveats.expires_at || narrowings_end.is_some_and(|end| next.0 > end);
    let status = match &own {
        Some(o) if o.revoked_at.is_some() => {
            format!(
                "revoked at Pulse {}",
                o.revoked_at.map(|p| p.0).unwrap_or(0)
            )
        }
        Some(o) if o.suspended_by.is_some() => format!(
            "suspended by {}",
            o.suspended_by.map(|a| a.to_string()).unwrap_or_default()
        ),
        _ if stopped => "stopped by an ancestor".into(),
        _ if expired => "expired".into(),
        _ if narrowed => "active, narrowed".into(),
        _ => "active".into(),
    };
    Ok(json!({
        "agent": id,
        "did": did(ledger, &id)?,
        "account": charter.agent,
        "keyless": keyless,
        "signed_for_by": signer,
        "principal": charter.principal,
        "backer": charter.backer,
        "card": charter.identity.as_ref().map(|i| i.card),
        "depth": charter.lineage.depth(),
        "lineage_root": charter.lineage.root,
        "lineage_path": charter.lineage.path,
        "status": status,
        "narrowing": own.as_ref().and_then(|o| o.narrowing.as_ref()).map(narrowing_json),
        "created_at": charter.created_at,
        "expires_at": charter.caveats.expires_at,
        "may_spawn": charter.caveats.may_delegate,
        "max_per_payment_usd": charter.caveats.max_per_payment.to_string(),
        "burst_usd": charter.caveats.burst.to_string(),
        "refill_usd_per_pulse": charter.caveats.rate.to_string(),
        "can_spend_next_pulse_usd": Amount(now).to_string(),
        "kept_after_children": {
            "burst_usd": Amount(burst).to_string(),
            "refill_usd_per_pulse": Amount(rate).to_string(),
        },
        "budget_remaining_usd": Amount(remaining).to_string(),
        "allotments": charter.allotments,
        "lineage": lineage,
    }))
}

/// A narrowing as `sion agent show` prints it, amounts in USD.
fn narrowing_json(n: &sion_exec::charter::Narrowing) -> Value {
    json!({
        "payees": n.payees,
        "max_per_payment_usd": n.max_per_payment.to_string(),
        "expires_at": n.expires_at,
        "may_spawn": n.may_delegate,
    })
}

/// An agent's DID: `did:sion:<chain identity>:<agent id>`.
pub fn did(ledger: &dyn Ledger, agent: &ObjectId) -> Result<String, CliError> {
    let status = ledger.status()?;
    let chain = status["chain"].as_str().unwrap_or("unknown");
    Ok(format!("did:sion:{chain}:{agent}"))
}

/// Loads a charter.
pub fn identity(ledger: &dyn Ledger, id: ObjectId) -> Result<Charter, CliError> {
    match ledger.object(id)?.map(|o| o.data) {
        Some(ObjectData::Charter(charter)) => Ok(charter),
        Some(_) => Err(CliError::Usage(format!("{id} is not an agent or charter"))),
        None => Err(CliError::Usage(format!("no object {id}"))),
    }
}

/// The commitment of a card file, or zero for none.
fn read_card(path: Option<&std::path::Path>) -> Result<Hash32, CliError> {
    match path {
        None => Ok(Hash32::ZERO),
        Some(path) => {
            let bytes = std::fs::read(path)
                .map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?;
            Ok(card_commitment(&bytes))
        }
    }
}

/// Makes sure a wallet named `name` has a seed file, creating one with a
/// random seed if it has none.
pub(crate) fn ensure_wallet(name: &str) -> Result<(), CliError> {
    if wallet_dir().join(format!("{name}.seed")).exists() {
        return Ok(());
    }
    create_wallet(name).map(|_| ())
}

pub(crate) fn print_receipt(session: &Session<'_>, view: &sion_node::TxView) {
    println!(
        "Pulse {}: {}",
        view.pulse,
        crate::describe::receipt(&session.names(), &view.effects)
    );
}

fn parse_id(text: &str, what: &str) -> Result<ObjectId, CliError> {
    Hash32::from_hex(text.trim())
        .map(ObjectId::from_hash)
        .map_err(|_| CliError::Usage(format!("{what} must be a 64-digit hex ID")))
}

/// A decimal amount with at most six places: `12.5` → 12,500,000 micro-units.
pub fn parse_amount(text: &str) -> Result<Amount, CliError> {
    let text = text.trim();
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    if whole.is_empty() && fraction.is_empty() || fraction.len() > 6 {
        return Err(CliError::Usage(format!(
            "{text:?} is not an amount with at most 6 decimals"
        )));
    }
    let whole: u128 = if whole.is_empty() {
        0
    } else {
        whole
            .parse()
            .map_err(|_| CliError::Usage(format!("{text:?} is not an amount")))?
    };
    let mut padded = fraction.to_string();
    while padded.len() < 6 {
        padded.push('0');
    }
    let fraction: u128 = padded
        .parse()
        .map_err(|_| CliError::Usage(format!("{text:?} is not an amount")))?;
    Ok(Amount(whole * 1_000_000 + fraction))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_card_template_has_the_a2a_fields_and_a_sion_section() {
        let card = card_template("researcher", "finds papers", "https://example.org/a2a");
        for field in [
            "name",
            "description",
            "url",
            "version",
            "capabilities",
            "skills",
        ] {
            assert!(card.get(field).is_some(), "{field}");
        }
        assert!(card["sion"]["agent"]
            .as_str()
            .unwrap()
            .starts_with("did:sion:"));
    }

    #[test]
    fn amounts_parse_to_micro_units() {
        assert_eq!(parse_amount("12.5").unwrap(), Amount(12_500_000));
        assert_eq!(parse_amount(".000001").unwrap(), Amount(1));
        assert!(parse_amount("1.0000001").is_err());
        assert!(parse_amount("").is_err());
    }
}
