//! Scheduled jobs: a task an agent profile works on at a time, every
//! interval or on a cron expression. Each run is a session of its own,
//! under the profile's rules and the agent's charter, and what it said goes
//! to a channel: a file, a webhook or a Telegram chat.
//!
//! `sion agent scheduler` runs the jobs that are due, then sleeps until the
//! next one; with `--once` it runs what is due and stops, for a system's
//! own timer (cron, systemd) to call.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sion_agent::schedule::{rfc3339, When};

use crate::error::CliError;
use crate::runtime::SharedLedger;
use crate::session::wallet_dir;

/// The most runs a job remembers.
pub const KEPT_RUNS: usize = 20;

/// One run of a job.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    /// When it started (Unix seconds).
    pub at: u64,
    /// Its session.
    pub session: String,
    /// What it last said.
    pub said: String,
    /// Whether it reached its channel, or why not.
    pub delivered: String,
}

/// A scheduled job.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Job {
    /// Its name.
    pub name: String,
    /// The agent profile that runs it.
    pub profile: String,
    /// The task.
    pub task: String,
    /// When it runs.
    pub when: When,
    /// Where what it said goes: `file:<path>`, `webhook:<url>`,
    /// `telegram:<chat id>`, or `<messenger>:<chat>` ([`MESSENGERS`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deliver: Option<String>,
    /// When it runs next; None once it is over.
    pub next: Option<u64>,
    /// Its latest runs, newest last.
    #[serde(default)]
    pub runs: Vec<Run>,
}

/// The file jobs are kept in: `$SION_HOME/agents/schedules.json`.
pub fn path() -> PathBuf {
    wallet_dir()
        .parent()
        .map(|home| home.join("agents").join("schedules.json"))
        .unwrap_or_else(|| PathBuf::from(".sion/agents/schedules.json"))
}

/// Every job, by name.
pub fn all() -> Result<BTreeMap<String, Job>, CliError> {
    let path = path();
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|e| CliError::Usage(format!("{}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(CliError::Usage(format!("{}: {e}", path.display()))),
    }
}

/// Writes every job.
pub fn save_all(jobs: &BTreeMap<String, Job>) -> Result<(), CliError> {
    crate::profiles::write_json(&path(), jobs)
}

/// The current time, in Unix seconds.
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Checks a channel's description.
pub fn check_channel(channel: &str) -> Result<(), CliError> {
    match channel.split_once(':') {
        Some(("file", path)) if !path.is_empty() => Ok(()),
        Some(("webhook", url)) if url.starts_with("https://") || url.starts_with("http://") => {
            Ok(())
        }
        Some(("telegram", chat)) if chat.parse::<i64>().is_ok() => Ok(()),
        Some((messenger, chat)) if MESSENGERS.contains(&messenger) && !chat.is_empty() => Ok(()),
        _ => Err(CliError::Usage(format!(
            "a channel is file:<path>, webhook:<url>, telegram:<chat id>, or <messenger>:<chat> ({}), not {channel:?}",
            MESSENGERS.join(", ")
        ))),
    }
}

/// The messengers a job's answer can be sent through, to a chat of theirs,
/// with the bridge's credentials: those that can write to a chat the
/// bridge has not just been written from.
pub const MESSENGERS: [&str; 14] = [
    "slack",
    "discord",
    "matrix",
    "signal",
    "mattermost",
    "whatsapp",
    "line",
    "sms",
    "email",
    "imessage",
    "feishu",
    "lark",
    "nextcloud-talk",
    "google-chat",
];

/// Adds a job; returns it.
pub fn add(
    name: &str,
    profile: &str,
    task: &str,
    when: When,
    deliver: Option<String>,
    now: u64,
) -> Result<Job, CliError> {
    if !crate::profiles::valid_name(name) {
        return Err(CliError::Usage(format!(
            "{name:?} is not a job name: 1 to 32 lowercase letters, digits and hyphens"
        )));
    }
    crate::profiles::get(profile)?;
    if let Some(channel) = &deliver {
        check_channel(channel)?;
    }
    if let When::Cron(text) = &when {
        sion_agent::schedule::Cron::parse(text).map_err(CliError::Usage)?;
    }
    let next = when.next_after(now);
    if next.is_none() {
        return Err(CliError::Usage(format!(
            "the job would never run: {}",
            when.describe()
        )));
    }
    let job = Job {
        name: name.to_string(),
        profile: profile.to_string(),
        task: task.to_string(),
        when,
        deliver,
        next,
        runs: Vec::new(),
    };
    let mut jobs = all()?;
    jobs.insert(name.to_string(), job.clone());
    save_all(&jobs)?;
    Ok(job)
}

/// One line about a job.
pub fn describe(job: &Job) -> String {
    format!(
        "{}: {} runs {:?} {}; next {}{}",
        job.name,
        job.profile,
        job.task,
        job.when.describe(),
        job.next.map(rfc3339).unwrap_or_else(|| "never".into()),
        job.deliver
            .as_ref()
            .map(|d| format!("; to {d}"))
            .unwrap_or_default()
    )
}

/// Sends `message` to `channel`.
pub fn deliver(channel: &str, message: &Value) -> Result<(), String> {
    let (kind, target) = channel
        .split_once(':')
        .ok_or_else(|| format!("{channel:?} is not a channel"))?;
    match kind {
        "file" => {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(target)
                .map_err(|e| format!("{target}: {e}"))?;
            writeln!(file, "{message}").map_err(|e| format!("{target}: {e}"))
        }
        "webhook" => {
            // The person named the address, so a private one is allowed.
            let client = sion_http::Client::new(sion_http::Options {
                allow_private: true,
                ..sion_http::Options::default()
            })
            .map_err(|e| e.to_string())?;
            client
                .post_json(target, &[], message.to_string().as_bytes(), 64 << 10)
                .map(|_| ())
                .map_err(|e| e.to_string())
        }
        "telegram" => {
            let chat: i64 = target
                .parse()
                .map_err(|_| format!("{target:?} is not a chat id"))?;
            crate::channels::Telegram::from_env()?
                .send(chat, message["said"].as_str().unwrap_or_default())
        }
        // Any other messenger, to a chat of its: with the bridge's
        // credentials, and for those that take webhooks, a listener of no
        // use on a port of its own.
        "google-chat" if std::env::var_os("GOOGLE_CHAT_CREDENTIALS").is_none() => Err(
            "google-chat sends a message of its own only with a service account's key (GOOGLE_CHAT_CREDENTIALS)".into(),
        ),
        other => {
            let messenger = crate::messengers::for_bridge(
                other,
                &crate::messengers::Options {
                    allow: vec![target.to_string()],
                    listen: "127.0.0.1:0".into(),
                    ..crate::messengers::Options::default()
                },
            )?;
            messenger.send(target, message["said"].as_str().unwrap_or_default())
        }
    }
}

/// Runs every job due at `now`; returns what to print.
pub fn run_due(ledger: SharedLedger, now: u64) -> Result<Vec<String>, CliError> {
    let mut jobs = all()?;
    let due: Vec<String> = jobs
        .values()
        .filter(|j| j.next.is_some_and(|n| n <= now))
        .map(|j| j.name.clone())
        .collect();
    let mut lines = Vec::new();
    for name in due {
        let Some(job) = jobs.get_mut(&name) else {
            continue;
        };
        lines.push(format!("job {}: {}", job.name, job.task));
        let (session, said) = match crate::profiles::get(&job.profile)
            .and_then(|profile| profile.start(ledger.clone(), &job.task))
        {
            Ok((saved, shown)) => {
                lines.extend(shown.into_iter().map(|l| format!("  {l}")));
                (saved.state.id.clone(), crate::runtime::last_words(&saved))
            }
            Err(e) => (String::new(), format!("the run failed: {e}")),
        };
        let message = json!({
            "job": job.name,
            "profile": job.profile,
            "session": session,
            "at": rfc3339(now),
            "said": said,
        });
        let delivered = match &job.deliver {
            Some(channel) => match deliver(channel, &message) {
                Ok(()) => format!("delivered to {channel}"),
                Err(e) => format!("not delivered to {channel}: {e}"),
            },
            None => "no channel".into(),
        };
        lines.push(format!("job {}: {said} ({delivered})", job.name));
        job.runs.push(Run {
            at: now,
            session,
            said,
            delivered,
        });
        if job.runs.len() > KEPT_RUNS {
            let extra = job.runs.len() - KEPT_RUNS;
            job.runs.drain(..extra);
        }
        job.next = job.when.next_after(now);
    }
    save_all(&jobs)?;
    Ok(lines)
}

/// Runs jobs as they fall due, until stopped; with `once`, only those due
/// now. Between runs it sleeps until the next job, checking at least every
/// `poll` seconds for jobs added meanwhile.
pub fn scheduler(ledger: SharedLedger, once: bool, poll: u64) -> Result<(), CliError> {
    loop {
        for line in run_due(ledger.clone(), now())? {
            println!("{line}");
        }
        if once {
            return Ok(());
        }
        let next = all()?.values().filter_map(|j| j.next).min();
        let wait = match next {
            Some(next) => next.saturating_sub(now()).clamp(1, poll.max(1)),
            None => poll.max(1),
        };
        std::thread::sleep(std::time::Duration::from_secs(wait));
    }
}

/// `sion agent schedule …`
#[derive(Debug, clap::Subcommand)]
pub enum ScheduleCommand {
    /// Schedule a task for a profile: once (--at), every interval
    /// (--every) or on a cron expression (--cron, UTC).
    Add {
        /// The job's name (lowercase letters, digits and hyphens).
        name: String,
        /// The agent profile that runs it.
        profile: String,
        /// The task, in words.
        #[arg(long)]
        task: String,
        /// Every interval: 90s, 15m, 2h, 1d.
        #[arg(long, conflicts_with_all = ["at", "cron"])]
        every: Option<String>,
        /// Once, at a time like 2026-11-02T09:00:00Z.
        #[arg(long, conflicts_with = "cron")]
        at: Option<String>,
        /// On a cron expression: minute hour day month weekday, in UTC.
        #[arg(long)]
        cron: Option<String>,
        /// Where what it said goes: file:<path>, webhook:<url>,
        /// telegram:<chat id> (TELEGRAM_BOT_TOKEN), or <messenger>:<chat>
        /// with the bridge's credentials: slack, discord, matrix, signal,
        /// mattermost, whatsapp, line, sms, email, imessage, feishu, lark,
        /// nextcloud-talk or google-chat (with a service account).
        #[arg(long)]
        deliver: Option<String>,
    },
    /// The scheduled jobs, with their next runs and latest results.
    List,
    /// Remove a job.
    Remove {
        /// Its name.
        name: String,
    },
}

/// Runs a schedule command.
pub fn run(command: ScheduleCommand) -> Result<(), CliError> {
    match command {
        ScheduleCommand::Add {
            name,
            profile,
            task,
            every,
            at,
            cron,
            deliver,
        } => {
            let now = now();
            let when = match (every, at, cron) {
                (Some(every), None, None) => When::Every {
                    seconds: sion_agent::schedule::parse_duration(&every)
                        .map_err(CliError::Usage)?,
                    from: now,
                },
                (None, Some(at), None) => {
                    When::At(sion_agent::schedule::parse_rfc3339(&at).map_err(CliError::Usage)?)
                }
                (None, None, Some(cron)) => When::Cron(cron),
                _ => return Err(CliError::Usage("say when: --every, --at or --cron".into())),
            };
            let job = add(&name, &profile, &task, when, deliver, now)?;
            println!("{}", describe(&job));
        }
        ScheduleCommand::List => {
            let jobs = all()?;
            if jobs.is_empty() {
                println!("no scheduled jobs in {}", path().display());
            }
            for job in jobs.values() {
                println!("{}", describe(job));
                if let Some(run) = job.runs.last() {
                    println!(
                        "  last run {}: session {}: {} ({})",
                        rfc3339(run.at),
                        run.session,
                        run.said,
                        run.delivered
                    );
                }
            }
        }
        ScheduleCommand::Remove { name } => {
            let mut jobs = all()?;
            if jobs.remove(&name).is_none() {
                return Err(CliError::Usage(format!("no scheduled job {name}")));
            }
            save_all(&jobs)?;
            println!("removed job {name}");
        }
    }
    Ok(())
}
