//! Channels: how people reach an agent profile from outside the command
//! line, through an HTTP endpoint (for webhooks and other programs), a
//! messenger ([`crate::messengers`]: Telegram, Slack, Discord, Matrix,
//! Signal), or a gateway on another machine ([`crate::gateway`]).
//!
//! Each conversation is a session, continued message by message. A session
//! that waits for a person's approval is answered from the same channel:
//! `/approve`, or `/decline <why>`. `/new` starts over, and `/status` says
//! where the session stands. The HTTP endpoint needs a bearer token: the
//! machine's, which reaches every profile, or one of a profile's own
//! ([`add_token`]), kept only as its hash, which reaches that profile and
//! decides what it waits on only if it was made to. A messenger answers
//! only the chats it is told to, and lets only the approvers it is given
//! decide.
//!
//! The HTTP endpoint streams when asked to (`"stream": true`): the reply
//! comes as Server-Sent Events while the agent works, `session` as its run
//! begins, `delta` with what it wrote since the last, `tool` with a tool it
//! calls, runs or waits on, and `done` with what the whole answer would
//! have been.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};

use serde_json::{json, Value};

use crate::error::CliError;
use crate::profiles::Profile;
use crate::runtime::{Reply, SharedLedger, Spoken};
use crate::session::wallet_dir;

/// The longest message accepted or sent, in bytes.
pub const MAX_MESSAGE: usize = 16 << 10;

/// The longest text Telegram takes in one message, in characters.
const TELEGRAM_TEXT: usize = 4_000;

fn conversations_path() -> PathBuf {
    wallet_dir()
        .parent()
        .map(|home| home.join("agents").join("conversations.json"))
        .unwrap_or_else(|| PathBuf::from(".sion/agents/conversations.json"))
}

/// The session each conversation is in, by key, and other channel state.
fn conversations() -> Result<BTreeMap<String, String>, CliError> {
    let path = conversations_path();
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|e| CliError::Usage(format!("{}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(CliError::Usage(format!("{}: {e}", path.display()))),
    }
}

/// Held while the conversations' file changes: a bridge answers chats on
/// threads of their own.
static CHANGING: Mutex<()> = Mutex::new(());

fn remember(key: &str, value: Option<&str>) -> Result<(), CliError> {
    let _changing = CHANGING.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = conversations()?;
    match value {
        Some(value) => all.insert(key.to_string(), value.to_string()),
        None => all.remove(key),
    };
    crate::profiles::write_json(&conversations_path(), &all)
}

/// A channel's own state under `key` (a messenger's cursor, say).
pub fn state(key: &str) -> Result<Option<String>, CliError> {
    Ok(conversations()?.get(key).cloned())
}

/// Sets, or with `None` clears, a channel's own state under `key`.
pub fn set_state(key: &str, value: Option<&str>) -> Result<(), CliError> {
    remember(key, value)
}

/// A profile's channel token, kept as its hash: whoever holds the token may
/// talk to the profile through a channel, and with `approve`, decide what
/// its sessions wait on.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChannelToken {
    /// A short id, to name it when removing it.
    pub id: String,
    /// The profile.
    pub profile: String,
    /// The profile's agent. A gateway routes the profile only to a host
    /// that proves this agent's key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<sion_core::ids::ObjectId>,
    /// The token's hash.
    pub hash: sion_core::hash::Hash32,
    /// Whether it may approve or decline.
    pub approve: bool,
    /// When it was made (seconds since the Unix epoch).
    pub created: u64,
}

fn tokens_path() -> PathBuf {
    wallet_dir()
        .parent()
        .map(|home| home.join("agents").join("channel-tokens.json"))
        .unwrap_or_else(|| PathBuf::from(".sion/agents/channel-tokens.json"))
}

fn hash_token(secret: &str) -> sion_core::hash::Hash32 {
    sion_core::hash::hash_domain("sion/agent/channel-token/v1", secret.as_bytes())
}

/// The channel tokens of every profile on this machine.
pub fn tokens() -> Result<Vec<ChannelToken>, CliError> {
    let path = tokens_path();
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|e| CliError::Usage(format!("{}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(CliError::Usage(format!("{}: {e}", path.display()))),
    }
}

/// Makes a channel token for `profile` (a profile name: on a gateway, the
/// profile lives on another machine), whose agent is `agent`. Returns its
/// record and the token, which is shown once and kept only as its hash.
pub fn add_token(
    profile: &str,
    agent: sion_core::ids::ObjectId,
    approve: bool,
) -> Result<(ChannelToken, String), CliError> {
    if !crate::profiles::valid_name(profile) {
        return Err(CliError::Usage(format!(
            "{profile:?} is not a profile name"
        )));
    }
    let mut all = tokens()?;
    // One agent per profile name, so that every token of a profile means
    // the same agent.
    if let Some(other) = all
        .iter()
        .find(|t| t.profile == profile && t.agent.is_some_and(|a| a != agent))
    {
        return Err(CliError::Usage(format!(
            "profile {profile}'s token {} names agent {}: remove its tokens first, or use another name",
            other.id,
            other.agent.map(|a| a.0.short()).unwrap_or_default()
        )));
    }
    let mut bytes = [0u8; 32];
    rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, &mut bytes);
    let secret = format!("sion_ch_{}", hex::encode(bytes));
    let hash = hash_token(&secret);
    let record = ChannelToken {
        id: hash.to_hex()[..8].to_string(),
        profile: profile.to_string(),
        agent: Some(agent),
        hash,
        approve,
        created: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    };
    all.push(record.clone());
    crate::profiles::write_json(&tokens_path(), &all)?;
    Ok((record, secret))
}

/// Whether a token on this machine says that `profile` is `agent`'s: what
/// a gateway asks before it routes the profile.
pub fn names_agent(profile: &str, agent: &sion_core::ids::ObjectId) -> Result<bool, CliError> {
    Ok(tokens()?
        .iter()
        .any(|t| t.profile == profile && t.agent.as_ref() == Some(agent)))
}

/// Removes every token of `profile`. Returns how many there were.
pub fn remove_tokens(profile: &str) -> Result<usize, CliError> {
    let mut all = tokens()?;
    let before = all.len();
    all.retain(|t| t.profile != profile);
    let removed = before - all.len();
    if removed > 0 {
        crate::profiles::write_json(&tokens_path(), &all)?;
    }
    Ok(removed)
}

/// Removes `profile`'s token `id`. Returns whether there was one.
pub fn remove_token(profile: &str, id: &str) -> Result<bool, CliError> {
    let mut all = tokens()?;
    let before = all.len();
    all.retain(|t| !(t.profile == profile && t.id == id));
    let removed = all.len() < before;
    if removed {
        crate::profiles::write_json(&tokens_path(), &all)?;
    }
    Ok(removed)
}

/// What a presented token may do with a profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Access {
    /// Nothing.
    None,
    /// Start and continue sessions.
    Message,
    /// That, and approve or decline.
    Approve,
}

/// What `presented` may do with `profile`: the machine's `admin` token may
/// do everything with every profile; a token of the profile, what it was
/// made for.
pub fn access(profile: &str, presented: Option<&str>, admin: Option<&str>) -> Access {
    let Some(presented) = presented else {
        return Access::None;
    };
    if admin.is_some_and(|admin| same_token(presented, admin)) {
        return Access::Approve;
    }
    let hash = hash_token(presented);
    match tokens()
        .unwrap_or_default()
        .iter()
        .find(|t| t.profile == profile && t.hash == hash)
    {
        Some(t) if t.approve => Access::Approve,
        Some(_) => Access::Message,
        None => Access::None,
    }
}

/// Why a request with `presented` (or none) may not reach a profile.
pub fn refusal(presented: Option<&str>) -> &'static str {
    match presented {
        Some(_) => "this token does not reach this profile",
        None => "a bearer token is needed",
    }
}

/// Whether a message decides what a session waits on.
pub fn is_decision(text: &str) -> bool {
    matches!(
        text.split_whitespace().next(),
        Some("/approve" | "/decline")
    )
}

/// Whether a message is a command only those who may decide may give:
/// approving or declining, and choosing the model, the chat's instructions
/// or its tools.
pub fn is_privileged(text: &str) -> bool {
    matches!(
        text.split_whitespace().next(),
        Some("/approve" | "/decline" | "/model" | "/system" | "/tools")
    )
}

/// Why a token that may only talk to a profile may not send `text`, if it
/// may not: it decides, or chooses for the conversation.
pub fn refused_for_talk(text: &str) -> Option<&'static str> {
    if is_decision(text) {
        Some("this token may talk to the profile, not approve or decline")
    } else if is_privileged(text) {
        Some("this token may talk to the profile, not choose its model, instructions or tools")
    } else {
        None
    }
}

/// The most characters of what a chat asks of its sessions (`/system`).
pub const MAX_CHAT_INSTRUCTIONS: usize = 4_000;

/// What a chat asks of its sessions (`/model`, `/system`, `/tools`), kept
/// with the channel's state: a new session starts as it says, and the
/// commands change the current one too.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChatSettings {
    /// The model its sessions start with, one of the profile's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// What its sessions are asked, after the operator's instructions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    /// The only tools its sessions may use, of those the profile gives.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<std::collections::BTreeSet<String>>,
}

impl ChatSettings {
    /// What conversation `key` asks of its sessions.
    pub fn of(key: &str) -> Result<ChatSettings, CliError> {
        Ok(state(&format!("{key}:settings"))?
            .and_then(|kept| serde_json::from_str(&kept).ok())
            .unwrap_or_default())
    }

    /// Keeps them for conversation `key`.
    pub fn keep(&self, key: &str) -> Result<(), CliError> {
        let kept = (self != &ChatSettings::default())
            .then(|| serde_json::to_string(self).unwrap_or_default());
        set_state(&format!("{key}:settings"), kept.as_deref())
    }
}

/// The commands a conversation takes, one line each.
pub const COMMANDS: &str = "/new: start over, in a new session
/status: where the session stands
/stop: stop what the agent is doing
/approve, /decline <why>: decide the call the session waits on
/retry: answer the last message again
/undo: forget the last message and its answer
/compact: shorten the session, its start kept as a summary
/usage: the tokens and turns the session used
/sessions: this conversation's sessions; /resume <id>: go back to one
/title <name>: name the session
/model: the models; /model <number>: choose one
/system: what this chat asks of the agent; /system <text>, /system off
/tools: the tools it may use here; /tools <name> …, /tools all
/whoami: who you are here, and what you may do
/help: these commands";

/// What a conversation's next message leads to: the session and the reply.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Answer {
    /// The session, if there is one.
    pub session: Option<String>,
    /// What to say back.
    pub reply: String,
    /// What happened in the session, one line each.
    pub lines: Vec<String>,
    /// What the agent said in the run, if one ran: the reply without what
    /// it waits for or why it stopped.
    pub said: String,
    /// Whether the run was stopped before it ended (`/stop`).
    pub stopped: bool,
    /// The files the agent shared in the run (`share_file`), to go with
    /// the reply.
    pub files: Vec<crate::files::File>,
}

/// The runs going on, by conversation: what stops each.
static RUNNING: Mutex<BTreeMap<String, Arc<AtomicBool>>> = Mutex::new(BTreeMap::new());

/// A run's hold on its conversation, given up when dropped.
struct Claim {
    key: String,
    stop: Arc<AtomicBool>,
}

impl Drop for Claim {
    fn drop(&mut self) {
        let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
        if running
            .get(&self.key)
            .is_some_and(|stop| Arc::ptr_eq(stop, &self.stop))
        {
            running.remove(&self.key);
        }
    }
}

/// Holds conversation `key` for a run: `None` if one goes on in it.
fn claim(key: &str) -> Option<Claim> {
    let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
    if running.contains_key(key) {
        return None;
    }
    let stop = Arc::new(AtomicBool::new(false));
    running.insert(key.to_string(), stop.clone());
    Some(Claim {
        key: key.to_string(),
        stop,
    })
}

/// Stops the run going on in conversation `key` before its next model or
/// tool call: whether one was going on.
pub fn stop(key: &str) -> bool {
    let running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
    match running.get(key) {
        Some(stop) => {
            stop.store(true, Ordering::SeqCst);
            true
        }
        None => false,
    }
}

/// Whether a run goes on in conversation `key`.
pub fn running(key: &str) -> bool {
    RUNNING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains_key(key)
}

/// Takes the next message of conversation `key` with `profile`: a command,
/// or text that starts a session or continues it.
pub fn converse(
    ledger: SharedLedger,
    profile: &Profile,
    key: &str,
    text: &str,
) -> Result<Answer, CliError> {
    converse_watched(ledger, profile, key, text, crate::runtime::Hooks::default())
}

/// The earlier sessions of conversation `key`, oldest first.
fn history(key: &str) -> Result<Vec<String>, CliError> {
    Ok(state(&format!("{key}:history"))?
        .and_then(|kept| serde_json::from_str(&kept).ok())
        .unwrap_or_default())
}

/// Puts `id` among the earlier sessions of conversation `key`, the last.
fn keep_in_history(key: &str, id: &str) -> Result<(), CliError> {
    let mut sessions = history(key)?;
    sessions.retain(|s| s != id);
    sessions.push(id.to_string());
    // The last fifty.
    let from = sessions.len().saturating_sub(50);
    let kept = serde_json::to_string(&sessions[from..]).unwrap_or_default();
    set_state(&format!("{key}:history"), Some(&kept))
}

/// A session in a line: its id, its title or task, and its last words.
fn session_line(saved: &crate::runtime::Saved) -> String {
    let short = |text: &str, n: usize| -> String {
        let line = text.lines().next().unwrap_or_default();
        if line.chars().count() > n {
            format!("{}…", line.chars().take(n).collect::<String>())
        } else {
            line.to_string()
        }
    };
    let name = saved.title.clone().unwrap_or_else(|| {
        short(
            &saved
                .state
                .messages
                .first()
                .map(|m| m.text())
                .unwrap_or_default(),
            40,
        )
    });
    format!(
        "{} {name}: {}",
        &saved.state.id[..saved.state.id.len().min(8)],
        short(&crate::runtime::last_words(saved), 60)
    )
}

/// [`converse`], the session's run given `hooks`: watched (what shows the
/// person that the agent is at work, and its reply as it is written),
/// stopped (`/stop`) and steered (what they say meanwhile). A command is
/// answered without a run, except `/retry` and decisions, which run.
pub fn converse_watched(
    ledger: SharedLedger,
    profile: &Profile,
    key: &str,
    text: &str,
    hooks: crate::runtime::Hooks<'_>,
) -> Result<Answer, CliError> {
    converse_with(ledger, profile, key, text, &[], hooks)
}

/// [`converse_watched`], what the person said coming with `pictures`,
/// which the model is shown.
pub fn converse_with(
    ledger: SharedLedger,
    profile: &Profile,
    key: &str,
    text: &str,
    pictures: &[sion_agent::Picture],
    hooks: crate::runtime::Hooks<'_>,
) -> Result<Answer, CliError> {
    let text = text.trim();
    if text.len() > MAX_MESSAGE {
        return Err(CliError::Usage(format!(
            "a message has at most {MAX_MESSAGE} bytes"
        )));
    }
    let current = conversations()?.get(key).cloned();
    let pending = current
        .as_deref()
        .and_then(|id| crate::runtime::Saved::load(id).ok())
        .and_then(|saved| saved.state.pending.clone().map(|p| (saved, p)));
    let reply = |session: Option<String>, reply: String| Answer {
        session,
        reply,
        ..Answer::default()
    };
    let (command, rest) = text.split_once(' ').unwrap_or((text, ""));
    let rest = rest.trim();
    // The current session, for the commands that need one.
    let session = || -> Result<Option<crate::runtime::Saved>, CliError> {
        current
            .as_deref()
            .map(crate::runtime::Saved::load)
            .transpose()
    };
    let no_session = || reply(None, "No session yet: send a task.".into());
    // What only reads may be asked while a run goes on; the rest waits.
    match command {
        "/stop" => {
            return Ok(reply(
                current,
                if stop(key) {
                    "Stopping.".into()
                } else {
                    "Nothing is running.".into()
                },
            ))
        }
        "/status" if running(key) => {
            return Ok(reply(current, "Working on it: /stop stops it.".into()))
        }
        _ => {}
    }
    let reads = matches!(command, "/status" | "/help" | "/usage" | "/sessions");
    let claim = match (reads, claim(key)) {
        (true, _) => None,
        (false, Some(claim)) => Some(claim),
        (false, None) => return Ok(reply(
            current,
            "The agent is still at work in this conversation: wait for its answer, or /stop it."
                .into(),
        )),
    };
    let mut settings = ChatSettings::of(key)?;
    let caller = hooks.stop;
    let stopped = move || {
        claim
            .as_ref()
            .is_some_and(|claim| claim.stop.load(Ordering::SeqCst))
            || caller.is_some_and(|stop| stop())
    };
    let decision = match command {
        "/new" => {
            if let Some(id) = &current {
                keep_in_history(key, id)?;
            }
            remember(key, None)?;
            return Ok(reply(
                None,
                "Starting over: the next message begins a new session.".into(),
            ));
        }
        "/status" => {
            return Ok(match &current {
                Some(id) => {
                    let saved = crate::runtime::Saved::load(id)?;
                    reply(Some(id.clone()), crate::runtime::last_words(&saved))
                }
                None => reply(None, "No session yet: send a task.".into()),
            })
        }
        "/help" => return Ok(reply(current, COMMANDS.to_string())),
        "/usage" => {
            let Some(saved) = session()? else {
                return Ok(no_session());
            };
            let state = &saved.state;
            return Ok(reply(
                current,
                format!(
                    "This session: {} turns, {} input and {} output tokens, {} messages{}.",
                    state.turns_taken,
                    state.usage.input,
                    state.usage.output,
                    state.messages.len(),
                    match state.compactions {
                        0 => String::new(),
                        1 => ", compacted once".to_string(),
                        n => format!(", compacted {n} times"),
                    }
                ),
            ));
        }
        "/sessions" => {
            let mut lines = Vec::new();
            if let Some(id) = &current {
                if let Ok(saved) = crate::runtime::Saved::load(id) {
                    lines.push(format!("now: {}", session_line(&saved)));
                }
            }
            for id in history(key)?.iter().rev() {
                if let Ok(saved) = crate::runtime::Saved::load(id) {
                    lines.push(session_line(&saved));
                }
            }
            return Ok(reply(
                current,
                if lines.is_empty() {
                    "No sessions yet: send a task.".into()
                } else {
                    format!("{}\n/resume <id> goes back to one.", lines.join("\n"))
                },
            ));
        }
        "/resume" => {
            // Only this conversation's own sessions.
            let earlier = history(key)?;
            let found: Vec<&String> = earlier
                .iter()
                .filter(|id| !rest.is_empty() && id.starts_with(rest))
                .collect();
            return Ok(match found.as_slice() {
                [id] => {
                    let saved = crate::runtime::Saved::load(id)?;
                    if let Some(now) = &current {
                        keep_in_history(key, now)?;
                    }
                    remember(key, Some(id))?;
                    reply(
                        Some((*id).clone()),
                        format!("Back in session {}.", session_line(&saved)),
                    )
                }
                [] => reply(
                    current,
                    "No earlier session of this conversation has that id: /sessions lists them."
                        .into(),
                ),
                _ => reply(current, "That names more than one session.".into()),
            });
        }
        "/title" => {
            let Some(mut saved) = session()? else {
                return Ok(no_session());
            };
            saved.title = (!rest.is_empty()).then(|| rest.chars().take(80).collect());
            saved.save()?;
            return Ok(reply(
                current,
                match &saved.title {
                    Some(title) => format!("The session is called {title}."),
                    None => "The session has no name now.".into(),
                },
            ));
        }
        "/undo" => {
            let Some(mut saved) = session()? else {
                return Ok(no_session());
            };
            return Ok(match saved.undo() {
                Some(left) if left > 0 => {
                    saved.save()?;
                    reply(
                        current,
                        "Undone: the last message and its answer are forgotten.".into(),
                    )
                }
                Some(_) => {
                    if let Some(id) = &current {
                        keep_in_history(key, id)?;
                    }
                    remember(key, None)?;
                    reply(
                        None,
                        "Undone: nothing is left, so the next message begins a new session.".into(),
                    )
                }
                None => reply(current, "Nothing to undo.".into()),
            });
        }
        "/compact" => {
            let Some(mut saved) = session()? else {
                return Ok(no_session());
            };
            let before = saved.state.messages.len();
            return Ok(
                if sion_agent::session::compact_now(&mut saved.state, saved.limits.keep_last) {
                    saved.save()?;
                    reply(
                        current,
                        format!(
                            "Compacted: {before} messages are {} now, the earlier ones a summary.",
                            saved.state.messages.len()
                        ),
                    )
                } else {
                    reply(current, "Nothing to compact yet.".into())
                },
            );
        }
        "/model" => {
            let mut saved = session().ok().flatten();
            let models = saved
                .as_ref()
                .map(|s| s.models.clone())
                .unwrap_or_else(|| profile.models.clone());
            if rest.is_empty() {
                let listed: Vec<String> = profile
                    .models
                    .iter()
                    .enumerate()
                    .map(|(at, spec)| {
                        let mark = if models.first() == Some(spec) {
                            " (now)"
                        } else {
                            ""
                        };
                        format!("{}. {spec}{mark}", at + 1)
                    })
                    .collect();
                return Ok(reply(
                    current,
                    format!("{}\n/model <number> chooses one.", listed.join("\n")),
                ));
            }
            // One of the profile's own models, by number or by name.
            let chosen = rest
                .parse::<usize>()
                .ok()
                .and_then(|n| n.checked_sub(1))
                .and_then(|at| profile.models.get(at))
                .or_else(|| profile.models.iter().find(|spec| spec.as_str() == rest))
                .cloned();
            let Some(chosen) = chosen else {
                return Ok(reply(
                    current,
                    "That is not one of this profile's models: /model lists them.".into(),
                ));
            };
            return Ok(match saved.as_mut() {
                Some(saved) => {
                    // Each model keeps where its script had got to.
                    let mut placed: Vec<(String, Option<usize>)> = saved
                        .models
                        .iter()
                        .enumerate()
                        .map(|(at, spec)| {
                            (spec.clone(), saved.positions.get(at).copied().flatten())
                        })
                        .collect();
                    let at = placed.iter().position(|(spec, _)| spec == &chosen);
                    let first = at.map_or((chosen.clone(), Some(0)), |at| placed.remove(at));
                    placed.insert(0, first);
                    saved.models = placed.iter().map(|(spec, _)| spec.clone()).collect();
                    saved.positions = placed.into_iter().map(|(_, at)| at).collect();
                    saved.save()?;
                    settings.model = Some(chosen.clone());
                    settings.keep(key)?;
                    reply(current, format!("This session uses {chosen} now."))
                }
                None => {
                    settings.model = Some(chosen.clone());
                    settings.keep(key)?;
                    reply(
                        current,
                        format!("The next session here starts with {chosen}."),
                    )
                }
            });
        }
        "/system" => {
            let mut saved = session()?;
            let said = match rest {
                "" => {
                    return Ok(reply(
                        current,
                        match &settings.instructions {
                            Some(asked) => format!("This chat asks: {asked}"),
                            None => "This chat asks nothing beyond the operator's instructions: /system <text> asks something.".into(),
                        },
                    ))
                }
                "off" => {
                    settings.instructions = None;
                    "This chat asks nothing beyond the operator's instructions now.".to_string()
                }
                asked if asked.chars().count() > MAX_CHAT_INSTRUCTIONS => {
                    return Ok(reply(
                        current,
                        format!("That is more than {MAX_CHAT_INSTRUCTIONS} characters."),
                    ))
                }
                asked => {
                    settings.instructions = Some(asked.to_string());
                    "Asked: this session and the next ones here take it after the operator's instructions.".to_string()
                }
            };
            settings.keep(key)?;
            if let Some(saved) = saved.as_mut() {
                saved.chat_instructions = settings.instructions.clone();
                saved.save()?;
            }
            return Ok(reply(current, said));
        }
        "/tools" => {
            let mut saved = session()?;
            let said =
                match rest {
                    "" => return Ok(reply(
                        current,
                        match &settings.tools {
                            Some(only) => format!(
                                "Only these, of the tools the profile gives: {}.",
                                only.iter().cloned().collect::<Vec<_>>().join(", ")
                            ),
                            None => {
                                "Every tool the profile gives: /tools <name> … names the only ones."
                                    .into()
                            }
                        },
                    )),
                    "all" => {
                        settings.tools = None;
                        "Every tool the profile gives, now.".to_string()
                    }
                    names => {
                        let only: std::collections::BTreeSet<String> = names
                            .split(|c: char| c == ',' || c.is_whitespace())
                            .filter(|name| !name.is_empty())
                            .map(str::to_string)
                            .collect();
                        let said = format!(
                            "Only these, of the tools the profile gives, here: {}.",
                            only.iter().cloned().collect::<Vec<_>>().join(", ")
                        );
                        settings.tools = Some(only);
                        said
                    }
                };
            settings.keep(key)?;
            if let Some(saved) = saved.as_mut() {
                saved.only_tools = settings.tools.clone();
                saved.save()?;
            }
            return Ok(reply(current, said));
        }
        "/retry" => {
            let Some(mut saved) = session()? else {
                return Ok(no_session());
            };
            if !saved.rewind() {
                return Ok(reply(current, "Nothing to answer again.".into()));
            }
            let lines = saved.run_watched(ledger, held(hooks, &stopped))?;
            remember(key, Some(&saved.state.id))?;
            return Ok(answered(&saved, lines));
        }
        "/approve" => Some(Reply::Approve),
        "/decline" => Some(Reply::Decline(if rest.is_empty() {
            "declined".to_string()
        } else {
            rest.to_string()
        })),
        _ => None,
    };
    let hooks = held(hooks, &stopped);
    let (saved, lines) = match (decision, &pending, &current) {
        (Some(decision), Some((saved, _)), _) => {
            crate::runtime::resume_watched(ledger, &saved.state.id, Some(decision), hooks)?
        }
        (Some(_), None, _) => return Ok(reply(current, "Nothing waits for a decision.".into())),
        (None, Some((saved, waiting)), _) => {
            return Ok(reply(
                Some(saved.state.id.clone()),
                format!(
                    "The session waits for your decision on {} {}: /approve, or /decline <why>.",
                    waiting.tool, waiting.input
                ),
            ))
        }
        (None, None, Some(id)) => crate::runtime::resume_watched(
            ledger,
            id,
            Some(Reply::Show(text.into(), pictures.to_vec())),
            hooks,
        )?,
        (None, None, None) => profile.start_in(ledger, text, pictures, hooks, &settings)?,
    };
    remember(key, Some(&saved.state.id))?;
    Ok(answered(&saved, lines))
}

/// `hooks`, the run also stopped by `stopped` ([`stop`]).
fn held<'a>(
    hooks: crate::runtime::Hooks<'a>,
    stopped: &'a (dyn Fn() -> bool + Sync),
) -> crate::runtime::Hooks<'a> {
    crate::runtime::Hooks {
        stop: Some(stopped),
        ..hooks
    }
}

/// What a session's run comes to, for whoever wrote to it: with the
/// files the agent shared, those that can still be read.
fn answered(saved: &crate::runtime::Saved, lines: Vec<String>) -> Answer {
    let mut lines = lines;
    let files = match &saved.workspace {
        Some(root) => saved
            .shared
            .iter()
            .filter_map(|path| match crate::files::File::read(root, path) {
                Ok(file) => Some(file),
                Err(why) => {
                    lines.push(format!("(not shared: {why})"));
                    None
                }
            })
            .collect(),
        None => Vec::new(),
    };
    Answer {
        session: Some(saved.state.id.clone()),
        reply: crate::runtime::last_words(saved),
        lines,
        said: saved.said.clone(),
        stopped: matches!(saved.ended, Some(sion_agent::Ended::Stopped)),
        files,
    }
}

/// What a streamed answer sends: its Server-Sent Events, as they come.
pub type Pieces = mpsc::Receiver<Vec<u8>>;

/// An event of a stream as Server-Sent Events frame it: named, or a plain
/// `data:` line.
pub fn event(name: Option<&str>, data: &Value) -> Vec<u8> {
    match name {
        Some(name) => format!("event: {name}\ndata: {data}\n\n"),
        None => format!("data: {data}\n\n"),
    }
    .into_bytes()
}

/// What a run said, for a stream that can only add to what it sent: the
/// text sent so far, from which only what is new goes.
#[derive(Debug, Default)]
pub struct Growing {
    spoken: Spoken,
    sent: String,
    /// Whether the text changed rather than grew (an answer dropped
    /// partway): nothing more goes until the end.
    off: bool,
}

impl Growing {
    /// Takes `seen`: the text that is new, if any.
    pub fn take(&mut self, seen: &sion_agent::Seen) -> Option<String> {
        use sion_agent::Seen;
        match seen {
            Seen::Text(text) => self.spoken.add(text),
            Seen::Answered(stop) => self.spoken.end(*stop == sion_agent::Stop::MaxTokens),
            Seen::Dropped => self.spoken.drop_current(),
            _ => return None,
        }
        if self.off {
            return None;
        }
        let text = self.spoken.text();
        match text.strip_prefix(self.sent.as_str()) {
            Some("") => None,
            Some(more) => {
                let more = more.to_string();
                self.sent = text;
                Some(more)
            }
            None => {
                self.off = true;
                None
            }
        }
    }

    /// What of `reply` is still to send: what follows the text sent, or,
    /// if the reply does not go on from it, all of it, a paragraph apart.
    pub fn rest(&self, reply: &str) -> String {
        match reply.strip_prefix(self.sent.as_str()) {
            Some(more) => more.to_string(),
            None => format!("\n\n{reply}"),
        }
    }
}

/// Answers `request` with Server-Sent Events, each piece written and sent
/// on as it comes (HTTP/1.1 chunks), until the pieces end.
pub(crate) fn respond_streaming(request: tiny_http::Request, pieces: Pieces) {
    let mut out = request.into_writer();
    let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nTransfer-Encoding: chunked\r\n\r\n";
    if out
        .write_all(head.as_bytes())
        .and_then(|()| out.flush())
        .is_err()
    {
        return;
    }
    for piece in pieces {
        if piece.is_empty() {
            continue;
        }
        let sent = write!(out, "{:x}\r\n", piece.len())
            .and_then(|()| out.write_all(&piece))
            .and_then(|()| out.write_all(b"\r\n"))
            .and_then(|()| out.flush());
        if sent.is_err() {
            // The client went: the run goes on, and is kept.
            return;
        }
    }
    let _ = out.write_all(b"0\r\n\r\n").and_then(|()| out.flush());
}

/// Runs the next message of conversation `key` with `profile` on a thread
/// of its own, its run as Server-Sent Events: `session` with the session's
/// id as the run begins, `delta` with the text the agent wrote since the
/// last, `tool` with a tool it calls, runs or waits on, then `done` with
/// what [`converse`] would have answered (or `error`).
fn converse_streamed(
    ledger: SharedLedger,
    profile: Profile,
    key: String,
    text: String,
    pictures: Vec<sion_agent::Picture>,
    done: impl FnOnce(&Answer) -> Value + Send + 'static,
) -> Pieces {
    let (tell, pieces) = mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let told = |name: &str, data: Value| {
            let _ = tell.send(event(Some(name), &data));
        };
        match converse_events(ledger, &profile, &key, &text, &pictures, &told) {
            Ok(answer) => told("done", done(&answer)),
            Err(e) => told("error", json!({"error": e.to_string()})),
        }
    });
    pieces
}

/// [`converse_with`], told to `tell` as events while the agent works:
/// `session` (`{"session": id}`) when its run begins, `tool`
/// (`{"tool": name, "state": "calling" | "running" | "waiting"}`), and
/// `delta` (`{"text": more}`), what the reply gains; the deltas, put
/// together, are the reply, the last of them told before the answer is
/// returned.
pub fn converse_events(
    ledger: SharedLedger,
    profile: &Profile,
    key: &str,
    text: &str,
    pictures: &[sion_agent::Picture],
    tell: &(dyn Fn(&str, Value) + Sync),
) -> Result<Answer, CliError> {
    let growing = Mutex::new(Growing::default());
    let watch = |seen: &sion_agent::Seen| {
        use sion_agent::Seen;
        if let Seen::Began(session) = seen {
            return tell("session", json!({"session": session}));
        }
        let tool = match seen {
            Seen::Calling(tool) => Some((tool, "calling")),
            Seen::Running(tool) => Some((tool, "running")),
            Seen::Waiting(tool) => Some((tool, "waiting")),
            _ => None,
        };
        if let Some((tool, state)) = tool {
            return tell("tool", json!({"tool": tool, "state": state}));
        }
        let more = growing.lock().map(|mut g| g.take(seen)).ok().flatten();
        if let Some(more) = more {
            tell("delta", json!({"text": more}));
        }
    };
    let answer = converse_with(
        ledger,
        profile,
        key,
        text,
        pictures,
        crate::runtime::Hooks::watched(&watch),
    )?;
    let rest = growing
        .lock()
        .map(|g| g.rest(&answer.reply))
        .unwrap_or_default();
    if !rest.is_empty() {
        tell("delta", json!({"text": rest}));
    }
    Ok(answer)
}

/// Whether two tokens are equal, in time that does not depend on where
/// they differ.
fn same_token(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

fn respond(request: tiny_http::Request, status: u16, body: &Value) {
    let header = tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
        .expect("a valid header");
    let _ = request.respond(
        tiny_http::Response::from_string(body.to_string())
            .with_status_code(status)
            .with_header(header),
    );
}

/// Serves the HTTP channel on `listen` until stopped (or for `requests`
/// requests, if given), with the machine's `token`: see [`serve_with`].
pub fn serve(
    ledger: SharedLedger,
    listen: &str,
    token: &str,
    requests: Option<usize>,
) -> Result<(), CliError> {
    serve_with(ledger, listen, Some(token), requests)
}

/// Serves the HTTP channel on `listen` until stopped (or for `requests`
/// requests, if given): `POST /v1/profiles/<name>/messages` with
/// `{"text": …, "conversation": …, "files": [{"name": …, "media_type": …,
/// "base64": …}]}` (the pictures shown to the agent, every file put in its
/// workspace's inbox), answered with what the agent said and the `files` it
/// shared; `GET /v1/sessions/<id>`; and OpenAI's
/// `/v1/chat/completions` and `/v1/models` ([`crate::openai_api`]). Every
/// request needs `Authorization: Bearer <token>`: the machine's `admin`
/// token, which reaches every profile and every session, or one of the
/// profile's own, which reaches only it, and decides what it waits on only
/// if it was made to. Requests are handled one at a time.
pub fn serve_with(
    ledger: SharedLedger,
    listen: &str,
    admin: Option<&str>,
    requests: Option<usize>,
) -> Result<(), CliError> {
    if admin.is_some_and(|token| token.len() < 16) {
        return Err(CliError::Usage(
            "the channel's token has at least 16 characters".into(),
        ));
    }
    let server = tiny_http::Server::http(listen)
        .map_err(|e| CliError::Usage(format!("cannot listen on {listen}: {e}")))?;
    println!(
        "agent channel on http://{listen} (POST /v1/profiles/<name>/messages; OpenAI's /v1/chat/completions and /v1/models)"
    );
    // Requests are answered side by side, so that a `/stop` reaches a run
    // another request started.
    let at_once = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for (index, request) in server.incoming_requests().enumerate() {
            while at_once.load(std::sync::atomic::Ordering::SeqCst) >= AT_ONCE {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            at_once.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let (ledger, at_once) = (ledger.clone(), &at_once);
            scope.spawn(move || {
                answer_request(ledger, admin, request);
                at_once.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            });
            if requests.is_some_and(|most| index + 1 >= most) {
                break;
            }
        }
    });
    Ok(())
}

/// The most requests the HTTP channel answers at once: the next waits.
pub const AT_ONCE: usize = 32;

/// The longest message to the HTTP channel, in bytes, its files with it:
/// the text alone has at most [`MAX_MESSAGE`].
pub const MAX_HTTP_MESSAGE: u64 = 32 << 20;

/// What a message to the HTTP channel says to the agent and shows it: its
/// `text`, with a line for each of its `files` (put in `profile`'s
/// workspace), and the pictures among them.
fn sent_over_http(
    value: &Value,
    profile: &Profile,
) -> Result<(String, Vec<sion_agent::Picture>), String> {
    let mut text = value["text"].as_str().unwrap_or_default().to_string();
    let files = value["files"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(index, file)| crate::files::File::from_json(file, index))
        .collect::<Result<Vec<_>, _>>()?;
    let (pictures, told) = crate::files::take(
        &files,
        profile.workspace.as_deref(),
        &crate::runtime::new_id(),
    );
    for line in told {
        text = format!("{text}\n{line}").trim_start().to_string();
    }
    Ok((text, pictures))
}

/// What the HTTP channel answers for `answer`, in `conversation`.
fn answer_json(conversation: &str, answer: &Answer) -> Value {
    json!({
        "conversation": conversation,
        "session": answer.session,
        "said": answer.reply,
        "lines": answer.lines,
        "files": answer.files.iter().map(crate::files::File::to_json).collect::<Vec<_>>(),
    })
}

/// Answers one request to the HTTP channel.
fn answer_request(ledger: SharedLedger, admin: Option<&str>, mut request: tiny_http::Request) {
    let presented = request
        .headers()
        .iter()
        .find(|h| h.field.equiv("Authorization"))
        .and_then(|h| h.value.as_str().strip_prefix("Bearer ").map(str::to_string));
    let path = request
        .url()
        .split('?')
        .next()
        .unwrap_or_default()
        .to_string();
    let method = request.method().as_str().to_string();
    // A2A: cards are public, for the profiles the channel publishes.
    if method == "GET" && (path.ends_with("agent-card.json") || path == "/.well-known/agent.json") {
        let host = request
            .headers()
            .iter()
            .find(|h| h.field.equiv("Host"))
            .map(|h| h.value.as_str().to_string());
        return match crate::a2a::card_at(&path, host.as_deref()) {
            Some(card) => respond(request, 200, &card),
            None => respond(request, 404, &json!({"error": "no agent card here"})),
        };
    }
    if let (Some(name), "POST") = (
        path.strip_prefix("/v1/profiles/")
            .and_then(|p| p.strip_suffix("/a2a")),
        method.as_str(),
    ) {
        let name = name.to_string();
        let version = request
            .headers()
            .iter()
            .find(|h| h.field.equiv("A2A-Version"))
            .map(|h| h.value.as_str().to_string());
        let mut body = String::new();
        let read = request
            .as_reader()
            .take(MAX_HTTP_MESSAGE + 1)
            .read_to_string(&mut body);
        let served = if read.is_err() || body.len() as u64 > MAX_HTTP_MESSAGE {
            crate::openai_api::Served::Whole(crate::openai_api::Answered {
                status: 413,
                content_type: "application/json",
                body: json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32600, "message": "the request is too long"}}).to_string(),
            })
        } else {
            crate::a2a::answer(
                ledger.clone(),
                &name,
                &body,
                version.as_deref(),
                presented.as_deref(),
                admin,
            )
        };
        match served {
            crate::openai_api::Served::Whole(answered) => {
                let header =
                    tiny_http::Header::from_bytes(&b"Content-Type"[..], answered.content_type)
                        .expect("a valid header");
                let _ = request.respond(
                    tiny_http::Response::from_string(answered.body)
                        .with_status_code(answered.status)
                        .with_header(header),
                );
            }
            crate::openai_api::Served::Streamed(pieces) => respond_streaming(request, pieces),
        }
        return;
    }
    if crate::openai_api::is_openai(&path) {
        let mut body = String::new();
        let read = request
            .as_reader()
            .take(crate::openai_api::MAX_REQUEST + 1)
            .read_to_string(&mut body);
        let served = if read.is_err() || body.len() as u64 > crate::openai_api::MAX_REQUEST {
            crate::openai_api::Served::Whole(crate::openai_api::Answered {
                status: 413,
                content_type: "application/json",
                body: json!({"error": {"message": "the request is too long", "type": "invalid_request_error"}}).to_string(),
            })
        } else {
            crate::openai_api::answer(
                ledger.clone(),
                &method,
                &path,
                &body,
                presented.as_deref(),
                admin,
            )
        };
        match served {
            crate::openai_api::Served::Whole(answered) => {
                let header =
                    tiny_http::Header::from_bytes(&b"Content-Type"[..], answered.content_type)
                        .expect("a valid header");
                let _ = request.respond(
                    tiny_http::Response::from_string(answered.body)
                        .with_status_code(answered.status)
                        .with_header(header),
                );
            }
            crate::openai_api::Served::Streamed(pieces) => respond_streaming(request, pieces),
        }
        return;
    }
    let profile = path
        .strip_prefix("/v1/profiles/")
        .and_then(|p| p.strip_suffix("/messages"))
        .unwrap_or_default()
        .to_string();
    let allowed = if profile.is_empty() {
        // Sessions are the machine's: only its token reads them.
        if admin.is_some_and(|a| presented.as_deref().is_some_and(|p| same_token(p, a))) {
            Access::Approve
        } else {
            Access::None
        }
    } else {
        access(&profile, presented.as_deref(), admin)
    };
    if allowed == Access::None {
        let error = if profile.is_empty() && presented.is_some() {
            "only the machine's token reads sessions"
        } else {
            refusal(presented.as_deref())
        };
        respond(request, 401, &json!({ "error": error }));
    } else if let (Some(name), "POST") = (
        path.strip_prefix("/v1/profiles/")
            .and_then(|p| p.strip_suffix("/messages")),
        method.as_str(),
    ) {
        let mut body = String::new();
        let read = request
            .as_reader()
            .take(MAX_HTTP_MESSAGE + 1)
            .read_to_string(&mut body);
        if read.is_ok() && body.len() as u64 > MAX_HTTP_MESSAGE {
            return respond(
                request,
                413,
                &json!({"error": format!("a message has at most {} MB, its files with it", MAX_HTTP_MESSAGE >> 20)}),
            );
        }
        let parsed: Option<Value> = read.ok().and_then(|_| serde_json::from_str(&body).ok());
        let refused = parsed
            .as_ref()
            .filter(|_| allowed != Access::Approve)
            .and_then(|value| refused_for_talk(value["text"].as_str().unwrap_or_default()));
        match parsed {
            Some(_) if refused.is_some() => respond(request, 403, &json!({ "error": refused })),
            Some(value) if value["text"].is_string() && value["stream"] == true => {
                let conversation = value["conversation"]
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(crate::runtime::new_id);
                let key = format!("http:{name}:{conversation}");
                let sent = crate::profiles::get(name)
                    .map_err(|e| e.to_string())
                    .and_then(|profile| {
                        sent_over_http(&value, &profile).map(|sent| (profile, sent))
                    });
                match sent {
                    Ok((profile, (text, pictures))) => {
                        let pieces = converse_streamed(
                            ledger.clone(),
                            profile,
                            key,
                            text,
                            pictures,
                            move |answer| answer_json(&conversation, answer),
                        );
                        respond_streaming(request, pieces);
                    }
                    Err(e) => respond(request, 400, &json!({ "error": e })),
                }
            }
            Some(value) if value["text"].is_string() => {
                let conversation = value["conversation"]
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(crate::runtime::new_id);
                let key = format!("http:{name}:{conversation}");
                let answer = crate::profiles::get(name)
                    .map_err(|e| e.to_string())
                    .and_then(|profile| {
                        let (text, pictures) = sent_over_http(&value, &profile)?;
                        converse_with(
                            ledger.clone(),
                            &profile,
                            &key,
                            &text,
                            &pictures,
                            crate::runtime::Hooks::default(),
                        )
                        .map_err(|e| e.to_string())
                    });
                match answer {
                    Ok(answer) => respond(request, 200, &answer_json(&conversation, &answer)),
                    Err(e) => respond(request, 400, &json!({ "error": e })),
                }
            }
            _ => respond(
                request,
                400,
                &json!({"error": "send {\"text\": …} as JSON"}),
            ),
        }
    } else if let (Some(id), "GET") = (path.strip_prefix("/v1/sessions/"), method.as_str()) {
        match crate::runtime::Saved::load(id) {
            Ok(saved) => respond(
                request,
                200,
                &json!({
                    "session": saved.state.id,
                    "said": crate::runtime::last_words(&saved),
                    "ended": saved.ended,
                }),
            ),
            Err(e) => respond(request, 404, &json!({"error": e.to_string()})),
        }
    } else {
        respond(request, 404, &json!({"error": "not found"}));
    }
}

/// A message from a Telegram chat.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Update {
    /// The update's number.
    pub id: i64,
    /// The message's id in its chat.
    pub message: i64,
    /// The topic it is in, in a forum or a private chat with topics.
    pub topic: Option<i64>,
    /// The chat.
    pub chat: i64,
    /// Who wrote it (a Telegram user id), if Telegram says.
    pub from: Option<i64>,
    /// The message's text (a voice note's caption).
    pub text: String,
    /// The voice note or audio it is, if one: its file id and media type.
    pub voice: Option<(String, String)>,
    /// The picture or document it carries, if any.
    pub files: Vec<crate::messengers::Note>,
    /// Whether the chat is a private one with the bot.
    pub direct: bool,
    /// Whom it names: `@usernames` (lower-cased) and user ids, the writer
    /// of the message it answers included.
    pub mentions: Vec<String>,
}

/// A Telegram bot, through the Bot API.
#[derive(Clone)]
pub struct Telegram {
    client: sion_http::Client,
    base: String,
    token: String,
    me: std::sync::OnceLock<Vec<String>>,
}

impl std::fmt::Debug for Telegram {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the token.
        write!(f, "Telegram({})", self.base)
    }
}

impl Telegram {
    /// The bot whose token is in `TELEGRAM_BOT_TOKEN`, at
    /// `TELEGRAM_API_BASE` (default `https://api.telegram.org`).
    pub fn from_env() -> Result<Telegram, String> {
        let token = std::env::var("TELEGRAM_BOT_TOKEN")
            .ok()
            .filter(|t| !t.is_empty())
            .ok_or("set TELEGRAM_BOT_TOKEN")?;
        let base = std::env::var("TELEGRAM_API_BASE")
            .ok()
            .filter(|b| !b.is_empty())
            .unwrap_or_else(|| "https://api.telegram.org".into());
        let client = sion_http::Client::new(sion_http::Options {
            timeout: std::time::Duration::from_secs(75),
            // The person names the API's address.
            allow_private: true,
            ..sion_http::Options::default()
        })
        .map_err(|e| e.to_string())?;
        Ok(Telegram {
            client,
            base: base.trim_end_matches('/').to_string(),
            token,
            me: std::sync::OnceLock::new(),
        })
    }

    /// The names messages give the bot: its `@username`, lower-cased, and
    /// its id (`getMe`, asked once).
    pub fn me(&self) -> Result<Vec<String>, String> {
        if let Some(me) = self.me.get() {
            return Ok(me.clone());
        }
        let bot = self.call("getMe", &json!({}))?;
        let mut names = Vec::new();
        if let Some(username) = bot["username"].as_str() {
            names.push(format!("@{}", username.to_ascii_lowercase()));
        }
        if let Some(id) = bot["id"].as_i64() {
            names.push(id.to_string());
        }
        if names.is_empty() {
            return Err("Telegram did not say who the bot is".into());
        }
        Ok(self.me.get_or_init(|| names).clone())
    }

    /// Calls the Bot API's `method` with `body`; its `result`.
    pub(crate) fn call(&self, method: &str, body: &Value) -> Result<Value, String> {
        let url = format!("{}/bot{}/{method}", self.base, self.token);
        let fetched = self
            .client
            .post_json(&url, &[], body.to_string().as_bytes(), 1 << 20)
            // The URL holds the token: never show it.
            .map_err(|e| e.to_string().replace(&self.token, "<token>"))?;
        let value: Value = serde_json::from_slice(&fetched.body)
            .map_err(|e| format!("Telegram answered with something other than JSON: {e}"))?;
        if value["ok"] != true {
            return Err(format!(
                "Telegram refused {method}: {}",
                value["description"].as_str().unwrap_or("no reason given")
            ));
        }
        Ok(value["result"].clone())
    }

    /// Sends `text` to `chat`.
    pub fn send(&self, chat: i64, text: &str) -> Result<(), String> {
        self.send_in(chat, None, text).map(|_| ())
    }

    /// Sends `text` to `chat`, in `topic` if given; the message's id.
    pub fn send_in(&self, chat: i64, topic: Option<i64>, text: &str) -> Result<i64, String> {
        let text: String = text.chars().take(TELEGRAM_TEXT).collect();
        let mut body = json!({"chat_id": chat, "text": if text.is_empty() { "(nothing)".to_string() } else { text }});
        if let Some(topic) = topic {
            body["message_thread_id"] = json!(topic);
        }
        let sent = self.call("sendMessage", &body)?;
        Ok(sent["message_id"].as_i64().unwrap_or(0))
    }

    /// The text messages and voice notes after `offset`, waiting up to
    /// `timeout` seconds for one.
    pub fn updates(&self, offset: i64, timeout: u64) -> Result<Vec<Update>, String> {
        let result = self.call(
            "getUpdates",
            &json!({"offset": offset, "timeout": timeout, "allowed_updates": ["message"]}),
        )?;
        Ok(result
            .as_array()
            .map(|updates| {
                updates
                    .iter()
                    .filter_map(|u| {
                        let message = &u["message"];
                        let text = message["text"]
                            .as_str()
                            .or_else(|| message["caption"].as_str())
                            .unwrap_or_default()
                            .to_string();
                        // A voice note is Ogg Opus; an audio file says what
                        // it is.
                        let voice = match (&message["voice"], &message["audio"]) {
                            (voice, _) if voice.is_object() => voice["file_id"]
                                .as_str()
                                .map(|id| (id.to_string(), "audio/ogg".to_string())),
                            (_, audio) if audio.is_object() => {
                                audio["file_id"].as_str().map(|id| {
                                    (
                                        id.to_string(),
                                        audio["mime_type"]
                                            .as_str()
                                            .unwrap_or("audio/mpeg")
                                            .to_string(),
                                    )
                                })
                            }
                            _ => None,
                        };
                        // A photo, its largest size; a document as it was
                        // sent.
                        let mut files = Vec::new();
                        if let Some(photo) = message["photo"].as_array().and_then(|p| p.last()) {
                            if let Some(id) = photo["file_id"].as_str() {
                                files.push(crate::messengers::Note {
                                    id: id.to_string(),
                                    media_type: Some("image/jpeg".into()),
                                    name: None,
                                });
                            }
                        }
                        if let Some(id) = message["document"]["file_id"].as_str() {
                            files.push(crate::messengers::Note {
                                id: id.to_string(),
                                media_type: message["document"]["mime_type"]
                                    .as_str()
                                    .map(str::to_string),
                                name: message["document"]["file_name"]
                                    .as_str()
                                    .map(str::to_string),
                            });
                        }
                        if text.is_empty() && voice.is_none() && files.is_empty() {
                            return None;
                        }
                        // `@username`s in the text, people named without
                        // one, and the writer of the message answered.
                        let mut mentions: Vec<String> = text
                            .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '@'))
                            .filter(|word| word.len() > 1 && word.starts_with('@'))
                            .map(str::to_ascii_lowercase)
                            .collect();
                        for entity in message["entities"]
                            .as_array()
                            .into_iter()
                            .chain(message["caption_entities"].as_array())
                            .flatten()
                        {
                            if let Some(id) = entity["user"]["id"].as_i64() {
                                mentions.push(id.to_string());
                            }
                        }
                        if let Some(id) = message["reply_to_message"]["from"]["id"].as_i64() {
                            mentions.push(id.to_string());
                        }
                        // A topic's messages are answered in the topic.
                        let topic = (message["is_topic_message"] == true
                            || message["chat"]["type"] == "private")
                            .then(|| message["message_thread_id"].as_i64())
                            .flatten();
                        Some(Update {
                            id: u["update_id"].as_i64()?,
                            message: message["message_id"].as_i64().unwrap_or(0),
                            topic,
                            chat: message["chat"]["id"].as_i64()?,
                            from: message["from"]["id"].as_i64(),
                            text,
                            voice,
                            files,
                            direct: message["chat"]["type"] == "private",
                            mentions,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// The file `file_id`, as audio of `media_type` (or what its bytes
    /// show).
    pub fn file(
        &self,
        file_id: &str,
        media_type: Option<&str>,
    ) -> Result<crate::voice::Audio, String> {
        let file = self.call("getFile", &json!({"file_id": file_id}))?;
        let path = file["file_path"]
            .as_str()
            .ok_or("Telegram gave no path for the file")?;
        let url = format!("{}/file/bot{}/{path}", self.base, self.token);
        let fetched = self
            .client
            .get(&url, crate::voice::MAX_HEARD as u64)
            .map_err(|e| e.to_string().replace(&self.token, "<token>"))?;
        let media_type = media_type
            .map(str::to_string)
            .unwrap_or_else(|| crate::voice::Audio::sniff(&fetched.body).to_string());
        Ok(crate::voice::Audio::new(&media_type, fetched.body))
    }

    /// Sends `audio` to `chat` as a voice note, with `caption` if it fits
    /// (1,024 characters); a longer one follows as text.
    pub fn send_voice(
        &self,
        chat: i64,
        audio: &crate::voice::Audio,
        caption: &str,
    ) -> Result<(), String> {
        let fits = caption.chars().count() <= 1024;
        let mut form = crate::voice::Form::new().text("chat_id", &chat.to_string());
        if fits && !caption.is_empty() {
            form = form.text("caption", caption);
        }
        let (content_type, body) = form.file("voice", audio).finish();
        let url = format!("{}/bot{}/sendVoice", self.base, self.token);
        let response = self
            .client
            .send(
                "POST",
                &url,
                &[("Content-Type", content_type.as_str())],
                Some(&body),
            )
            .map_err(|e| e.to_string().replace(&self.token, "<token>"))?;
        let answer: Value =
            serde_json::from_slice(&response.read_all(1 << 20).map_err(|e| e.to_string())?)
                .unwrap_or(Value::Null);
        if answer["ok"] != true {
            return Err(format!(
                "Telegram refused sendVoice: {}",
                answer["description"].as_str().unwrap_or("no reason given")
            ));
        }
        if !fits {
            self.send(chat, caption)?;
        }
        Ok(())
    }
}

impl Telegram {
    /// Sends `file` to `chat`, in `topic` if given: a picture as a photo
    /// (up to Telegram's 10 MB for one), anything else as a document, with
    /// `caption` if it fits.
    pub fn send_file_in(
        &self,
        chat: i64,
        topic: Option<i64>,
        file: &crate::files::File,
        caption: &str,
    ) -> Result<(), String> {
        let photo = file.is_picture() && file.media_type != "image/gif";
        let (method, field) = if photo {
            ("sendPhoto", "photo")
        } else {
            ("sendDocument", "document")
        };
        let mut form = crate::voice::Form::new().text("chat_id", &chat.to_string());
        if let Some(topic) = topic {
            form = form.text("message_thread_id", &topic.to_string());
        }
        if !caption.is_empty() && caption.chars().count() <= 1024 {
            form = form.text("caption", caption);
        }
        let (content_type, body) = form
            .bytes(field, &file.name, &file.media_type, &file.bytes)
            .finish();
        let url = format!("{}/bot{}/{method}", self.base, self.token);
        let response = self
            .client
            .send(
                "POST",
                &url,
                &[("Content-Type", content_type.as_str())],
                Some(&body),
            )
            .map_err(|e| e.to_string().replace(&self.token, "<token>"))?;
        let answer: Value =
            serde_json::from_slice(&response.read_all(1 << 20).map_err(|e| e.to_string())?)
                .unwrap_or(Value::Null);
        if answer["ok"] != true {
            return Err(format!(
                "Telegram refused {method}: {}",
                answer["description"].as_str().unwrap_or("no reason given")
            ));
        }
        Ok(())
    }
}

/// Answers the chats in `allow` with `profile`, by long polling, until
/// stopped; with `once`, one batch of messages.
pub fn telegram(
    ledger: SharedLedger,
    profile: &Profile,
    allow: &[i64],
    once: bool,
) -> Result<(), CliError> {
    telegram_with(ledger, profile, allow, &[], once)
}

/// [`telegram`], where only `approvers` (Telegram user ids), if any are
/// given, may approve or decline.
pub fn telegram_with(
    ledger: SharedLedger,
    profile: &Profile,
    allow: &[i64],
    approvers: &[String],
    once: bool,
) -> Result<(), CliError> {
    if allow.is_empty() {
        return Err(CliError::Usage(
            "name the chats the bot answers: --allow <chat id>,…".into(),
        ));
    }
    let bot = Telegram::from_env().map_err(CliError::Usage)?;
    let allow: Vec<String> = allow.iter().map(i64::to_string).collect();
    crate::messengers::bridge(ledger, profile, &bot, &allow, approvers, once)
}

/// [`telegram`], as `options` say (pairing, voice notes).
pub fn telegram_options(
    ledger: SharedLedger,
    profile: &Profile,
    options: &crate::messengers::Options,
) -> Result<(), CliError> {
    let bot = Telegram::from_env().map_err(CliError::Usage)?;
    crate::messengers::bridge_with(ledger, profile, &bot, options)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_over_http_brings_its_files_to_the_workspace() {
        let root = std::env::temp_dir().join(format!("sion-http-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let profile = Profile {
            name: "lead".into(),
            wallet: "lead".into(),
            agent: sion_core::ids::ObjectId::from_hash(sion_core::hash::Hash32::ZERO),
            models: Vec::new(),
            autonomy: sion_agent::Autonomy::L3,
            memory: None,
            skills: None,
            workspace: Some(root.clone()),
            web: false,
            delegate: false,
            subagent_models: Vec::new(),
            adjudicator: None,
            reflect: false,
            shell: None,
            mcp: Vec::new(),
            storage: false,
            instructions: None,
            skill_approvals: true,
            screen: Default::default(),
            image_model: None,
            home_assistant: false,
        };
        let sent = json!({"text": "Look at these.", "files": [
            {"name": "plan.png", "base64": "iVBORw0KGgoAAA=="},
            {"name": "notes.txt", "media_type": "text/plain", "base64": "aGk="}
        ]});
        let (text, pictures) = sent_over_http(&sent, &profile).unwrap();
        assert_eq!(
            text,
            "Look at these.\n(A picture was sent, put in your workspace as inbox/plan.png: image/png, 10 bytes.)\n(A file was sent, put in your workspace as inbox/notes.txt: text/plain, 2 bytes.)"
        );
        assert_eq!(pictures.len(), 1);
        assert_eq!(std::fs::read(root.join("inbox/notes.txt")).unwrap(), b"hi");
        // Files alone are a message; bytes that are not base64 are refused.
        let (text, _) = sent_over_http(&json!({"files": [{"base64": "aGk="}]}), &profile).unwrap();
        assert!(text.starts_with("(A file was sent"), "{text}");
        assert!(
            sent_over_http(&json!({"text": "x", "files": [{"base64": "!"}]}), &profile).is_err()
        );
        let answer = Answer {
            reply: "Here.".into(),
            files: vec![crate::files::File {
                media_type: "text/plain".into(),
                name: "a.txt".into(),
                bytes: b"hi".to_vec(),
            }],
            ..Answer::default()
        };
        let answered = answer_json("c1", &answer);
        assert_eq!(answered["said"], "Here.");
        assert_eq!(
            answered["files"],
            json!([{"name": "a.txt", "media_type": "text/plain", "size": 2, "base64": "aGk="}])
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_stream_sends_what_is_new_and_the_rest_at_the_end() {
        use sion_agent::{Seen, Stop};
        let mut growing = Growing::default();
        assert_eq!(growing.take(&Seen::Asking), None);
        assert_eq!(
            growing.take(&Seen::Text("Let me ".into())).as_deref(),
            Some("Let me")
        );
        assert_eq!(
            growing.take(&Seen::Text("check.".into())).as_deref(),
            Some(" check.")
        );
        assert_eq!(growing.take(&Seen::Answered(Stop::ToolUse)), None);
        assert_eq!(growing.take(&Seen::Running("ledger_balance".into())), None);
        assert_eq!(
            growing.take(&Seen::Text("100 USD left.".into())).as_deref(),
            Some("\n\n100 USD left.")
        );
        growing.take(&Seen::Answered(Stop::EndTurn));
        assert_eq!(growing.rest("Let me check.\n\n100 USD left."), "");
        assert_eq!(
            growing.rest("Let me check.\n\n100 USD left.\n\nwaiting for your approval"),
            "\n\nwaiting for your approval"
        );
        // An answer dropped partway: nothing more until the end.
        let mut dropped = Growing::default();
        dropped.take(&Seen::Text("Half".into()));
        assert_eq!(dropped.take(&Seen::Dropped), None);
        assert_eq!(dropped.take(&Seen::Text("Whole.".into())), None);
        assert_eq!(dropped.rest("Whole."), "\n\nWhole.");
        assert_eq!(
            event(Some("delta"), &json!({"text": "Hi"})),
            b"event: delta\ndata: {\"text\":\"Hi\"}\n\n".to_vec()
        );
    }

    #[test]
    fn tokens_compare_whole() {
        assert!(same_token("a-long-enough-secret", "a-long-enough-secret"));
        assert!(!same_token("a-long-enough-secret", "a-long-enough-secreT"));
        assert!(!same_token("short", "shorter"));
    }
}
