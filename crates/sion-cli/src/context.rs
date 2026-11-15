//! A session's context from the command line: what its conversation let go
//! and looked back for, and how a way of keeping a conversation short
//! would have served sessions already recorded.
//!
//! A long session shortens its conversation ([`sion_agent::earlier`]):
//! old results are cut and the middle is replaced by notes. What it lets
//! go is kept in the session, so both commands here read the whole
//! conversation:
//!
//! - `show` says how a session's conversation stands: what it holds, what
//!   it let go, each shortening with who wrote its notes and how many names
//!   they no longer held, and what the agent looked back for.
//! - `measure` plays recorded sessions again under a way of keeping a
//!   conversation short ([`sion_agent::session::replay`]): no tool is
//!   called and the agent's model is not asked. At each point where the
//!   conversation would have been shortened it counts the names the agent
//!   went on to use, and how many of them the shortened conversation still
//!   held. It compares two ways on real sessions for the cost of their
//!   notes, or for nothing with mechanical notes.
//!
//! What `measure` counts is names (paths, identifiers, long numbers), in
//! one recorded run. It is a cheap first look, not a score of how the
//! agent would have done: that takes running tasks under a short context
//! (`sion agent improve run --short-context`).

use sion_agent::earlier::{Keeping, Replayed};
use sion_agent::policy::Policy;
use sion_agent::session::{Extras, Runtime};
use sion_agent::tools::Toolbox;

use crate::error::CliError;
use crate::runtime::Saved;

/// `sion agent context …`.
#[derive(Clone, Debug, clap::Subcommand)]
pub enum ContextCommand {
    /// How a session's conversation stands: what it holds, what it let go
    /// and keeps, each time it was shortened with who wrote its notes, and
    /// what the agent looked back for.
    Show {
        /// The session (its id, or a unique prefix of it).
        session: String,
    },
    /// Play recorded sessions again under a way of keeping a conversation
    /// short, and count, at each point where it would have been shortened,
    /// the names the agent went on to use and how many of them the
    /// shortened conversation still held. No tool is called and the
    /// agent's model is not asked.
    Measure {
        /// The sessions (ids, or unique prefixes).
        #[arg(required = true)]
        sessions: Vec<String>,
        /// Shorten past this many messages (and a thousand characters for
        /// each), rather than at each session's own limits.
        #[arg(long, value_name = "MESSAGES")]
        at: Option<usize>,
        /// A profile whose way of keeping conversations is measured: what
        /// its improvement loop kept (default: the runtime's own way).
        #[arg(long)]
        profile: Option<String>,
        /// Messages kept as they are at the end.
        #[arg(long = "keep-last", value_name = "MESSAGES")]
        keep_last: Option<usize>,
        /// Old results longer than this many characters are cut first.
        #[arg(long = "cut-over", value_name = "CHARACTERS")]
        cut_over: Option<usize>,
        /// How many of the names the notes do not hold are listed after
        /// them.
        #[arg(long)]
        names: Option<usize>,
        /// What the writer of the notes is told it is for.
        #[arg(long)]
        told: Option<String>,
        /// What it is asked, after the messages its notes replace.
        #[arg(long)]
        asked: Option<String>,
        /// The model that writes the notes (default: none: the mechanical
        /// notes, which cost nothing).
        #[arg(long = "notes-model", value_name = "MODEL")]
        notes_model: Option<String>,
    },
}

/// How recorded sessions are played again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct How {
    /// Messages past which the conversation is shortened, if not at each
    /// session's own limits.
    pub at: Option<usize>,
    /// How it is kept short.
    pub keeping: Keeping,
    /// The model that writes the notes, if one does.
    pub notes_model: Option<String>,
}

fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// How `saved`'s conversation stands, a line each.
pub fn lines(saved: &Saved) -> Vec<String> {
    let state = &saved.state;
    let earlier = &state.earlier;
    let short = &state.id[..state.id.len().min(16)];
    if state.compactions == 0 {
        return vec![format!(
            "session {short}: {}; its conversation was never shortened",
            count(state.messages.len(), "message", "messages")
        )];
    }
    let mut lines = Vec::new();
    let notes = earlier.notes.min(state.messages.len().saturating_sub(1));
    lines.push(format!(
        "session {short}: {} in its conversation{}; {} let go and kept as {} ({} characters){}",
        count(state.messages.len(), "message", "messages"),
        match notes {
            0 => String::new(),
            n => format!(", {n} of them the runtime's notes"),
        },
        count(earlier.messages.len(), "message", "messages"),
        if earlier.messages.len() == 1 {
            "it was"
        } else {
            "they were"
        },
        earlier.chars(),
        match earlier.dropped {
            0 => String::new(),
            n => format!(
                "; the {} oldest went for good when it reached what it keeps",
                n
            ),
        },
    ));
    if earlier.unkept {
        lines.push(
            "  it was shortened before what it let go was kept: its whole conversation is not \
             known"
                .into(),
        );
    } else if earlier.is_empty() {
        lines.push(format!(
            "  it was shortened {} and nothing it let go was kept: its whole conversation is \
             not known",
            count(state.compactions as usize, "time", "times")
        ));
    }
    if !earlier.cut.is_empty() {
        lines.push(format!(
            "  {} cut from messages still there, kept as {}",
            count(earlier.cut.len(), "result", "results"),
            if earlier.cut.len() == 1 {
                "it was"
            } else {
                "they were"
            }
        ));
    }
    for (at, span) in earlier.spans.iter().enumerate() {
        let mut line = format!(
            "  {}. let go messages {} to {}: notes by {}",
            at + 1,
            span.from,
            span.to,
            if span.by.is_empty() {
                "the runtime (mechanical)"
            } else {
                &span.by
            }
        );
        if span.asked_again {
            line.push_str(", asked again");
        }
        if !span.made_up.is_empty() {
            line.push_str(&format!(
                "; they named what is nowhere in what they replaced: {}",
                span.made_up.join(", ")
            ));
        }
        line.push_str(&format!(
            "; {} of what it had let go then held nowhere; {} characters became {}",
            count(span.not_held, "name", "names"),
            span.before,
            span.after
        ));
        lines.push(line);
    }
    if earlier.looked > 0 {
        lines.push(format!(
            "looked back {}{}:",
            count(earlier.looked as usize, "time", "times"),
            if (earlier.looks.len() as u64) < earlier.looked {
                format!(" (the last {} shown)", earlier.looks.len())
            } else {
                String::new()
            }
        ));
        for look in &earlier.looks {
            if look.read {
                lines.push(format!("  read {} again", look.asked));
                continue;
            }
            lines.push(format!(
                "  searched for {:?}: {}",
                look.asked,
                if look.found.is_empty() {
                    "nothing found".to_string()
                } else {
                    format!(
                        "found in {} {}",
                        if look.found.len() == 1 {
                            "message"
                        } else {
                            "messages"
                        },
                        look.found
                            .iter()
                            .map(usize::to_string)
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }
            ));
        }
    } else if earlier.holds() {
        lines.push("it never looked back".into());
    }
    if state.is_whole() {
        lines.push(format!(
            "the whole conversation can still be read: {}",
            count(state.whole().len(), "message", "messages")
        ));
    }
    lines
}

/// Plays `saved`'s whole conversation again as `how` says
/// ([`sion_agent::session::replay`]). An error if its conversation was
/// shortened and what it let go is not all kept.
pub fn measure(saved: &Saved, how: &How) -> Result<Replayed, CliError> {
    if !saved.state.is_whole() {
        return Err(CliError::Usage(format!(
            "session {}: its conversation was shortened and what it let go is not all kept",
            saved.state.id
        )));
    }
    how.keeping.check().map_err(CliError::Usage)?;
    let mut limits = saved.limits.clone();
    how.keeping.apply(&mut limits);
    limits.budget_tokens = None;
    limits.budget_micros = None;
    if let Some(at) = how.at {
        if at < 6 {
            return Err(CliError::Usage("--at is at least 6 messages".into()));
        }
        limits.compact_messages = at;
        limits.compact_chars = at.saturating_mul(1_000);
    }
    let writer = match how.notes_model.as_deref() {
        None | Some("none") => None,
        Some(spec) => Some(crate::runtime::model(spec, 0)?),
    };
    let tools = Toolbox::new();
    let runtime = Runtime {
        models: Vec::new(),
        tools: &tools,
        policy: Policy::default(),
        limits,
        system: String::new(),
        now: &|| 0,
        stopped: &|| false,
        watch: None,
        steer: None,
        checkpoint: None,
        verify: None,
        code: None,
        hooks: None,
        extras: Extras {
            summarizer: writer.as_deref(),
            ..Default::default()
        },
    };
    Ok(sion_agent::session::replay(&runtime, &saved.state.whole()))
}

fn percent(part: usize, of: usize) -> String {
    match (part * 100 + of / 2).checked_div(of) {
        Some(percent) => format!("{percent}%"),
        None => "none to count".to_string(),
    }
}

/// What a replay showed, a line each, under `name`.
pub fn measured_lines(name: &str, replayed: &Replayed) -> Vec<String> {
    let mut lines = Vec::new();
    if replayed.shortenings == 0 {
        lines.push(format!(
            "{name}: {}; never shortened under these limits",
            count(replayed.messages, "message", "messages")
        ));
        return lines;
    }
    lines.push(format!(
        "{name}: {}; shortened {}",
        count(replayed.messages, "message", "messages"),
        count(replayed.shortenings, "time", "times"),
    ));
    lines.push(format!(
        "  of {} its agent went on to use from what came before, the shortened conversation \
         still held {} ({}){}",
        count(replayed.needed, "name", "names"),
        replayed.held,
        percent(replayed.held, replayed.needed),
        match replayed.listed {
            0 => String::new(),
            n => format!("; {n} more it listed as let go, to be looked back for"),
        }
    ));
    if !replayed.lost.is_empty() {
        let shown: Vec<String> = replayed
            .lost
            .iter()
            .take(8)
            .map(|lost| format!("{} (message {})", lost.name, lost.used_at))
            .collect();
        let more = replayed.lost.len().saturating_sub(shown.len());
        lines.push(format!(
            "  not held, with the message that used each: {}{}",
            shown.join(", "),
            if more > 0 {
                format!(", and {more} more")
            } else {
                String::new()
            }
        ));
    }
    lines.push(format!(
        "  after a shortening its conversation was {} of its length before, on average",
        percent(replayed.after, replayed.before)
    ));
    lines
}

/// Runs `sion agent context …`.
pub fn run(command: ContextCommand) -> Result<(), CliError> {
    match command {
        ContextCommand::Show { session } => {
            for line in lines(&Saved::load(&session)?) {
                println!("{line}");
            }
            Ok(())
        }
        ContextCommand::Measure {
            sessions,
            at,
            profile,
            keep_last,
            cut_over,
            names,
            told,
            asked,
            notes_model,
        } => {
            let mut keeping = match &profile {
                Some(name) => {
                    crate::profiles::get(name)?;
                    crate::improve_loop::context(name)
                        .map(|kept| kept.keeping)
                        .unwrap_or_default()
                }
                None => Keeping::default(),
            };
            if let Some(keep_last) = keep_last {
                keeping.keep_last = keep_last;
            }
            if let Some(cut_over) = cut_over {
                keeping.cut_over = cut_over;
            }
            if let Some(names) = names {
                keeping.names = names;
            }
            if let Some(told) = told {
                keeping.told = told;
            }
            if let Some(asked) = asked {
                keeping.asked = asked;
            }
            keeping.check().map_err(CliError::Usage)?;
            let how = How {
                at,
                keeping,
                notes_model,
            };
            println!(
                "kept as: the last {} as they are, old results over {} characters cut first, {} \
                 listed{}",
                count(how.keeping.keep_last, "message", "messages"),
                how.keeping.cut_over,
                count(how.keeping.names, "name", "names"),
                match how.keeping.change_from(&Keeping::default()).as_str() {
                    "keeps its conversation as before" => " (the runtime's own way)".to_string(),
                    _ => String::new(),
                }
            );
            println!(
                "notes: {}",
                match how.notes_model.as_deref() {
                    None | Some("none") =>
                        "mechanical (--notes-model has a model write them)".to_string(),
                    Some(spec) => format!("written by {spec}"),
                }
            );
            let mut all = Replayed::default();
            let mut measured = 0;
            for id in &sessions {
                let saved = Saved::load(id)?;
                let short = saved.state.id[..saved.state.id.len().min(16)].to_string();
                match measure(&saved, &how) {
                    Ok(replayed) => {
                        for line in measured_lines(&format!("session {short}"), &replayed) {
                            println!("{line}");
                        }
                        all.add(&replayed);
                        measured += 1;
                    }
                    Err(why) => println!("passed over: {why}"),
                }
            }
            if measured > 1 {
                println!(
                    "in all: {}, {}; {} of {} held ({})",
                    count(measured, "session", "sessions"),
                    count(all.shortenings, "shortening", "shortenings"),
                    all.held,
                    count(all.needed, "name", "names"),
                    percent(all.held, all.needed)
                );
            }
            if all.tokens > 0 {
                println!("{} tokens used by the notes' writer", all.tokens);
            }
            if measured == 0 {
                return Err(CliError::Usage("no session could be measured".into()));
            }
            Ok(())
        }
    }
}
