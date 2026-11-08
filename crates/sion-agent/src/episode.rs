//! A case as an environment for reinforcement learning: the runtime, the
//! case's stand-ins and its checks, with the model's turns given from
//! outside, by the policy being trained.
//!
//! The case is run as it is evaluated ([`crate::evals::run_case_with`]):
//! the same loop, policy and journal, the stand-ins answering the calls
//! from their fixtures. Where the runtime would ask a model, it asks the
//! [`OutsidePolicy`] instead, over JSON Lines: it writes the request in
//! OpenAI's chat format,
//!
//! ```text
//! {"request": {"messages": [...], "tools": [...], "max_tokens": 4096}}
//! ```
//!
//! and reads the policy's turn, an assistant message in the same format
//! (with `tool_calls` for the tools it calls), or an error:
//!
//! ```text
//! {"message": {"role": "assistant", "content": "...", "tool_calls": [...]}}
//! {"error": "why"}
//! ```
//!
//! When the run ends, the episode's reward is the share of the case's
//! checks that held ([`reward`]), written last:
//!
//! ```text
//! {"done": {"reward": 1.0, "passed": true, "failures": [], "calls": 2, "ended": {...}}}
//! ```

use std::io::{BufRead, Read, Write};
use std::sync::Mutex;

use serde_json::{json, Value};

use crate::evals::{Case, CaseRun};
use crate::model::{Model, ModelError, Request, Response};
use crate::providers::{openai_messages, openai_response, openai_tools};
use crate::session::Limits;
use crate::tools::Toolbox;

/// The longest line read from the policy, in bytes.
pub const MAX_LINE: u64 = 4 << 20;

/// A model whose turns are given from outside, a line each.
pub struct OutsidePolicy {
    id: String,
    read: Mutex<Box<dyn BufRead + Send>>,
    write: Mutex<Box<dyn Write + Send>>,
}

impl std::fmt::Debug for OutsidePolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "OutsidePolicy({})", self.id)
    }
}

impl OutsidePolicy {
    /// A policy named `id` that is asked on `write` and answers on `read`.
    pub fn new(
        id: &str,
        read: impl BufRead + Send + 'static,
        write: impl Write + Send + 'static,
    ) -> OutsidePolicy {
        OutsidePolicy {
            id: id.to_string(),
            read: Mutex::new(Box::new(read)),
            write: Mutex::new(Box::new(write)),
        }
    }

    /// Writes one line to the policy.
    pub fn tell(&self, value: &Value) -> Result<(), String> {
        let mut out = self.write.lock().unwrap_or_else(|p| p.into_inner());
        writeln!(out, "{value}")
            .and_then(|()| out.flush())
            .map_err(|e| format!("the policy cannot be told: {e}"))
    }
}

/// The request as the policy is shown it.
pub fn request_line(request: &Request) -> Value {
    json!({"request": {
        "messages": openai_messages(&request.system, &request.messages, &request.images, ""),
        "tools": openai_tools(&request.tools),
        "max_tokens": request.max_tokens,
    }})
}

/// The policy's answer as a model's response.
pub fn response_of(answer: &Value, model: &str) -> Result<Response, ModelError> {
    if let Some(why) = answer.get("error") {
        return Err(ModelError::Refused(format!(
            "the policy failed: {}",
            why.as_str().map_or_else(|| why.to_string(), str::to_string)
        )));
    }
    let message = answer
        .get("message")
        .filter(|m| m.is_object())
        .ok_or_else(|| ModelError::Protocol("the policy's line has no message".into()))?;
    let calls = message["tool_calls"]
        .as_array()
        .is_some_and(|calls| !calls.is_empty());
    let finish =
        answer["finish_reason"]
            .as_str()
            .unwrap_or(if calls { "tool_calls" } else { "stop" });
    openai_response(&json!({
        "choices": [{"index": 0, "message": message, "finish_reason": finish}],
        "usage": answer.get("usage").cloned().unwrap_or_else(|| json!({})),
        "model": model,
    }))
}

impl Model for OutsidePolicy {
    fn id(&self) -> &str {
        &self.id
    }

    fn complete(&self, request: &Request) -> Result<Response, ModelError> {
        self.tell(&request_line(request))
            .map_err(ModelError::Transport)?;
        let mut line = String::new();
        {
            let mut read = self.read.lock().unwrap_or_else(|p| p.into_inner());
            let read: &mut dyn BufRead = &mut *read;
            let got = read
                .take(MAX_LINE)
                .read_line(&mut line)
                .map_err(|e| ModelError::Transport(format!("the policy's answer: {e}")))?;
            if got == 0 {
                return Err(ModelError::Transport("the policy closed its side".into()));
            }
        }
        let answer: Value = serde_json::from_str(&line)
            .map_err(|e| ModelError::Protocol(format!("the policy's line is not JSON: {e}")))?;
        response_of(&answer, &self.id)
    }
}

/// The episode's reward: the share of the case's checks that held, 1 when
/// it has none.
pub fn reward(case: &Case, run: &CaseRun) -> f64 {
    let total = case.checks.len();
    if total == 0 {
        return 1.0;
    }
    total.saturating_sub(run.failures.len()) as f64 / total as f64
}

/// What ends an episode, as the policy is told it.
pub fn done_line(case: &Case, run: &CaseRun) -> Value {
    json!({"done": {
        "case": case.id,
        "reward": reward(case, run),
        "passed": run.passed,
        "failures": run.failures,
        "calls": run.calls.len(),
        "ended": run.ended,
    }})
}

/// Runs `case` as an episode with `policy` as its model and only its
/// stand-ins as tools, `system` its system prompt: the run.
pub fn run(case: &Case, system: &str, policy: &dyn Model, limits: &Limits) -> CaseRun {
    // A policy that failed, or closed its side, is not waited for: what it
    // answers is its own to decide.
    let limits = Limits {
        patience: 0,
        ..limits.clone()
    };
    crate::evals::run_case_with(case, Toolbox::new(), &[policy], system, &limits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evals::{Answer, Check, Fixture};
    use crate::tools::Effect;
    use std::io::{BufReader, Cursor};
    use std::sync::Arc;

    /// What the policy was told, kept.
    #[derive(Clone, Default)]
    struct Told(Arc<Mutex<Vec<u8>>>);

    impl Write for Told {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn case() -> Case {
        Case {
            id: "pay-acme".into(),
            task: "Pay the invoice from Acme.".into(),
            tools: vec![
                Fixture {
                    name: "lookup".into(),
                    description: "Find a payee.".into(),
                    effect: Effect::Read,
                    answers: vec![Answer {
                        when: serde_json::from_value(json!({"supplier": "acme"})).unwrap(),
                        says: "acme-ltd".into(),
                        error: false,
                        outside: false,
                        confidential: false,
                    }],
                    otherwise: Some("no such supplier".into()),
                    command_field: None,
                },
                Fixture {
                    name: "pay".into(),
                    description: "Pay a payee.".into(),
                    effect: Effect::Spend,
                    answers: Vec::new(),
                    otherwise: Some("paid".into()),
                    command_field: None,
                },
            ],
            checks: vec![
                Check::Done,
                Check::Called {
                    tool: "pay".into(),
                    with: serde_json::from_value(json!({"to": "acme-ltd"})).unwrap(),
                },
                Check::Before {
                    first: "lookup".into(),
                    then: "pay".into(),
                },
            ],
            person: None,
        }
    }

    fn turns(lines: &[Value]) -> BufReader<Cursor<Vec<u8>>> {
        let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
        BufReader::new(Cursor::new(text.into_bytes()))
    }

    fn call(id: &str, name: &str, arguments: Value) -> Value {
        json!({"message": {"role": "assistant", "content": null, "tool_calls": [
            {"id": id, "type": "function", "function": {"name": name, "arguments": arguments.to_string()}}
        ]}})
    }

    #[test]
    fn a_policy_that_looks_up_then_pays_earns_the_whole_reward() {
        let told = Told::default();
        let policy = OutsidePolicy::new(
            "policy",
            turns(&[
                call("c1", "lookup", json!({"supplier": "acme"})),
                call("c2", "pay", json!({"to": "acme-ltd", "amount": "5"})),
                json!({"message": {"role": "assistant", "content": "Paid acme-ltd."}}),
            ]),
            told.clone(),
        );
        let case = case();
        let run = run(&case, "You pay invoices.", &policy, &Limits::default());
        assert!(run.passed, "{:?}", run.failures);
        assert_eq!(reward(&case, &run), 1.0);
        let done = done_line(&case, &run);
        assert_eq!(done["done"]["reward"], 1.0);
        assert_eq!(done["done"]["calls"], 2);
        // It was shown the case's tools only, in OpenAI's format, and the
        // stand-in's answer to its lookup.
        let told = String::from_utf8(told.0.lock().unwrap().clone()).unwrap();
        let requests: Vec<Value> = told
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(requests.len(), 3);
        let names: Vec<&str> = requests[0]["request"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["function"]["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["lookup", "pay"]);
        assert_eq!(requests[0]["request"]["messages"][0]["role"], "system");
        let second = requests[1]["request"]["messages"].as_array().unwrap();
        assert_eq!(second.last().unwrap()["role"], "tool");
        assert_eq!(second.last().unwrap()["content"], "acme-ltd");
    }

    #[test]
    fn a_policy_that_pays_without_looking_earns_part_and_one_that_fails_none() {
        let case = case();
        let hasty = OutsidePolicy::new(
            "hasty",
            turns(&[
                call("c1", "pay", json!({"to": "acme"})),
                json!({"message": {"role": "assistant", "content": "Paid."}}),
            ]),
            Told::default(),
        );
        let run = run(&case, "", &hasty, &Limits::default());
        assert!(!run.passed);
        assert!(
            (reward(&case, &run) - 1.0 / 3.0).abs() < 1e-9,
            "{:?}",
            run.failures
        );
        let failing = OutsidePolicy::new(
            "failing",
            turns(&[json!({"error": "out of memory"})]),
            Told::default(),
        );
        let run = super::run(&case, "", &failing, &Limits::default());
        assert_eq!(reward(&case, &run), 0.0, "{:?}", run.failures);
        let silent = OutsidePolicy::new("silent", turns(&[]), Told::default());
        let run = super::run(&case, "", &silent, &Limits::default());
        assert!(!run.passed);
        assert!(response_of(&json!({"nothing": 1}), "m").is_err());
    }
}
