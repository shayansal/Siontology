//! Cases from an agent's own work: a session whose outcome is known — its
//! check passed, or its owner said it went well or badly — becomes a case
//! that another version of the agent can be scored on.
//!
//! - **The task** is what the session was asked, its one message from a
//!   person. A session that was a conversation (more than one message from
//!   a person), or that was compacted (its first message is no longer the
//!   task as asked), is not taken.
//! - **The stand-ins** answer what the real tools answered: each call the
//!   session made, by its exact input. For any other input, a tool that
//!   only reads says that no answer was recorded, as an error, so a
//!   version that asks something else learns nothing it could not have
//!   learnt; a tool that changes something says `ok`.
//! - **The checks.** For a session that went well: the run finishes, makes
//!   every payment the session made with the same input, and makes some
//!   call of every other tool the session changed something with. For one
//!   that went badly: the run makes none of the changes that session made,
//!   by their exact inputs.
//!
//! What a harvested case cannot know is whether the session's way was the
//! only right one — a check that the same payment is made fails a better
//! route to the same end — and whether the world would have answered a
//! different call as the stand-ins do. Cases harvested are a floor under
//! what the agent did well, not a grader of what it might do better.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::evals::{Answer, Case, Check, Fixture};
use crate::model::{Block, Role};
use crate::session::State;
use crate::tools::{Effect, TOOLS_CALL, TOOLS_SEARCH};

/// The most text of a tool's answer a stand-in keeps.
pub const MAX_ANSWER: usize = 8 << 10;

/// The most answers a stand-in keeps for one tool.
pub const MAX_ANSWERS: usize = 50;

/// What a stand-in answers an input it has no recording for, if it reads.
pub const NO_RECORDING: &str = "no answer was recorded for this input";

/// A call a session made, with what came back.
#[derive(Clone, Debug, PartialEq)]
pub struct Recorded {
    /// The tool.
    pub tool: String,
    /// Its input.
    pub input: Value,
    /// What it returned.
    pub content: String,
    /// Whether it failed.
    pub is_error: bool,
    /// What the call could do, as the journal knew it (a read, if the
    /// journal did not keep it).
    pub effect: Effect,
    /// Whether what it returned came from outside the agent's trust.
    pub outside: bool,
}

/// How a session went, as its check or its owner says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// It did what it was asked.
    Good,
    /// It did something it should not have.
    Bad,
}

/// The task a session was asked: its one message from a person. None for
/// a conversation, a compacted session, or one with no such message.
pub fn task_of(state: &State) -> Result<String, String> {
    if state.compactions > 0 {
        return Err("it was compacted: its first message is no longer the task as asked".into());
    }
    let asked: Vec<String> = state
        .messages
        .iter()
        .filter(|m| m.role == Role::User)
        .map(|m| m.text())
        .filter(|t| !t.trim().is_empty())
        .collect();
    match asked.as_slice() {
        [task] => Ok(task.clone()),
        [] => Err("no one asked it anything".into()),
        _ => Err("it was a conversation, not one task".into()),
    }
}

/// The calls `state` made, each with what came back, in order: a call
/// through `tools_call` counted as the call it makes, `tools_search`
/// left out.
pub fn recorded(state: &State) -> Vec<Recorded> {
    let mut results: BTreeMap<&str, (&str, bool)> = BTreeMap::new();
    for message in &state.messages {
        for block in &message.content {
            if let Block::ToolResult {
                id,
                content,
                is_error,
                ..
            } = block
            {
                results.insert(id.as_str(), (content.as_str(), *is_error));
            }
        }
    }
    let mut calls = Vec::new();
    for message in &state.messages {
        for (id, name, input) in message.tool_uses() {
            if name == TOOLS_SEARCH {
                continue;
            }
            let (tool, input) = if name == TOOLS_CALL {
                (
                    input["name"].as_str().unwrap_or_default().to_string(),
                    input.get("input").cloned().unwrap_or_else(json_object),
                )
            } else {
                (name.to_string(), input.clone())
            };
            let Some((content, is_error)) = results.get(id).copied() else {
                continue;
            };
            let key = crate::journal::key(&state.id, &tool, &input);
            let entry = state.journal.entries().get(&key);
            calls.push(Recorded {
                effect: entry.map(|e| e.effect).unwrap_or(Effect::Read),
                outside: entry.is_some_and(|e| e.outcome.untrusted),
                tool,
                input,
                content: content.to_string(),
                is_error,
            });
        }
    }
    calls
}

fn json_object() -> Value {
    Value::Object(Map::new())
}

fn cut(text: &str) -> String {
    if text.len() <= MAX_ANSWER {
        return text.to_string();
    }
    let mut end = MAX_ANSWER;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

fn fields(input: &Value) -> Map<String, Value> {
    match input {
        Value::Object(map) => map.clone(),
        _ => Map::new(),
    }
}

/// A case named `id` from a session that was asked `task`, made `calls`
/// and went as `verdict` says. A session that went badly without changing
/// anything gives no check to make, and no case.
pub fn case_of(id: &str, task: &str, calls: &[Recorded], verdict: Verdict) -> Result<Case, String> {
    let mut fixtures: Vec<Fixture> = Vec::new();
    for call in calls {
        if call.tool.starts_with("skill_") || call.tool.is_empty() {
            continue;
        }
        let at = match fixtures.iter().position(|f| f.name == call.tool) {
            Some(at) => at,
            None => {
                fixtures.push(Fixture {
                    name: call.tool.clone(),
                    description: String::new(),
                    effect: call.effect,
                    answers: Vec::new(),
                    otherwise: None,
                });
                fixtures.len() - 1
            }
        };
        let fixture = &mut fixtures[at];
        fixture.effect = fixture.effect.max(call.effect);
        let when = fields(&call.input);
        if fixture.answers.len() < MAX_ANSWERS && !fixture.answers.iter().any(|a| a.when == when) {
            fixture.answers.push(Answer {
                when,
                says: cut(&call.content),
                error: call.is_error,
                outside: call.outside,
            });
        }
    }
    for fixture in &mut fixtures {
        if fixture.effect == Effect::Read {
            fixture.answers.push(Answer {
                when: Map::new(),
                says: NO_RECORDING.into(),
                error: true,
                outside: false,
            });
        }
    }
    let changes: Vec<&Recorded> = calls
        .iter()
        .filter(|c| c.effect > Effect::Read && !c.is_error && !c.tool.starts_with("skill_"))
        .collect();
    let mut checks = Vec::new();
    match verdict {
        Verdict::Good => {
            checks.push(Check::Done);
            let mut named: Vec<&str> = Vec::new();
            for change in &changes {
                if change.effect == Effect::Spend {
                    let check = Check::Called {
                        tool: change.tool.clone(),
                        with: fields(&change.input),
                    };
                    if !checks.contains(&check) {
                        checks.push(check);
                    }
                } else if !named.contains(&change.tool.as_str()) {
                    named.push(&change.tool);
                    checks.push(Check::Called {
                        tool: change.tool.clone(),
                        with: Map::new(),
                    });
                }
            }
        }
        Verdict::Bad => {
            for change in &changes {
                let check = Check::NotCalled {
                    tool: change.tool.clone(),
                    with: fields(&change.input),
                };
                if !checks.contains(&check) {
                    checks.push(check);
                }
            }
            if checks.is_empty() {
                return Err("it went badly without changing anything: there is nothing to check a run against".into());
            }
        }
    }
    let case = Case {
        id: id.to_string(),
        task: task.to_string(),
        tools: fixtures,
        checks,
    };
    crate::evals::check_cases(std::slice::from_ref(&case))?;
    Ok(case)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evals::{check, FixtureTool};
    use crate::model::Message;
    use crate::session::{Ended, Limits};
    use crate::tools::Tool;
    use serde_json::json;

    fn session() -> State {
        let mut state = State::start("s1", "Pay Acme's invoice 7, 40 USD.", &Limits::default());
        state.messages.push(Message {
            role: Role::Assistant,
            content: vec![Block::ToolUse {
                id: "c1".into(),
                name: "lookup_payee".into(),
                input: json!({"supplier": "acme"}),
                signature: None,
            }],
        });
        state.messages.push(Message {
            role: Role::User,
            content: vec![Block::ToolResult {
                id: "c1".into(),
                content: "acme is paid at acme-ltd".into(),
                is_error: false,
                images: Vec::new(),
            }],
        });
        state.messages.push(Message {
            role: Role::Assistant,
            content: vec![Block::ToolUse {
                id: "c2".into(),
                name: "ledger_pay".into(),
                input: json!({"to": "acme-ltd", "amount": "40"}),
                signature: None,
            }],
        });
        state.messages.push(Message {
            role: Role::User,
            content: vec![Block::ToolResult {
                id: "c2".into(),
                content: "paid".into(),
                is_error: false,
                images: Vec::new(),
            }],
        });
        let input = json!({"to": "acme-ltd", "amount": "40"});
        let key = crate::journal::key("s1", "ledger_pay", &input);
        state.journal.record(
            &key,
            crate::journal::Entry {
                tool: "ledger_pay".into(),
                input,
                effect: Effect::Spend,
                outcome: crate::tools::Outcome::ok("paid"),
                at: 0,
                notes: Vec::new(),
                dedup: None,
            },
        );
        state
    }

    #[test]
    fn a_session_that_went_well_becomes_a_case_its_own_calls_pass() {
        let state = session();
        let task = task_of(&state).unwrap();
        let calls = recorded(&state);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[1].effect, Effect::Spend);
        let case = case_of("paid-acme", &task, &calls, Verdict::Good).unwrap();
        assert_eq!(case.task, "Pay Acme's invoice 7, 40 USD.");
        // The read answers its recorded input, and only that.
        let lookup = FixtureTool(case.tools[0].clone());
        assert_eq!(
            lookup.call(&json!({"supplier": "acme"}), "k").content,
            "acme is paid at acme-ltd"
        );
        let other = lookup.call(&json!({"supplier": "mallory"}), "k");
        assert!(other.is_error && other.content == NO_RECORDING);
        // The session's own calls pass the checks; paying someone else
        // does not.
        let made: Vec<(String, Value)> = calls
            .iter()
            .map(|c| (c.tool.clone(), c.input.clone()))
            .collect();
        let done = Ended::Done {
            text: "Paid.".into(),
        };
        assert!(case.checks.iter().all(|c| check(c, &made, &done).is_none()));
        let wrong = vec![(
            "ledger_pay".to_string(),
            json!({"to": "mallory", "amount": "40"}),
        )];
        assert!(case
            .checks
            .iter()
            .any(|c| check(c, &wrong, &done).is_some()));
    }

    #[test]
    fn a_session_that_went_badly_checks_its_changes_are_not_made_again() {
        let state = session();
        let calls = recorded(&state);
        let case = case_of("bad", "Pay it.", &calls, Verdict::Bad).unwrap();
        assert_eq!(case.checks.len(), 1);
        assert!(matches!(&case.checks[0], Check::NotCalled { tool, .. } if tool == "ledger_pay"));
        // Nothing changed: nothing to check.
        let reads: Vec<Recorded> = calls
            .into_iter()
            .filter(|c| c.effect == Effect::Read)
            .collect();
        assert!(case_of("bad", "Pay it.", &reads, Verdict::Bad).is_err());
    }

    #[test]
    fn a_conversation_or_a_compacted_session_is_not_a_task() {
        let mut state = session();
        state.messages.push(Message::user("And the next one?"));
        assert!(task_of(&state).unwrap_err().contains("conversation"));
        let mut state = session();
        state.compactions = 1;
        assert!(task_of(&state).is_err());
    }
}
