//! A model that plays a script: for tests, demos, evaluations and
//! replaying a session. It makes no network call.
//!
//! A step may depend on the conversation: `when` plays it only if the
//! conversation so far contains a text, `unless` only if it does not. A
//! step whose condition fails is passed over, and the next one is tried in
//! the same call. That lets a script stand in for a model that follows the
//! instructions it reads, such as a skill's: it does only what its script
//! says, under the conditions its script names.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use serde::Deserialize;
use serde_json::Value;

use crate::model::{Block, Model, ModelError, Request, Response, Stop, Usage};

type Step = Box<dyn Fn(&Request) -> Result<Response, ModelError> + Send + Sync>;

/// A step and the condition under which it plays.
struct Guarded {
    when: Option<String>,
    unless: Option<String>,
    step: Step,
}

impl Guarded {
    /// Whether it plays in a conversation whose text, in lower case, is
    /// `text`.
    fn applies(&self, text: &str) -> bool {
        let holds =
            |needle: &Option<String>| needle.as_ref().map(|n| text.contains(&n.to_lowercase()));
        holds(&self.when).unwrap_or(true) && !holds(&self.unless).unwrap_or(false)
    }
}

/// A model that answers each call with the next step of its script whose
/// condition holds.
pub struct Scripted {
    id: String,
    steps: Mutex<VecDeque<Guarded>>,
    seen: Mutex<Vec<Request>>,
    /// Steps used so far, played or passed over.
    used: AtomicUsize,
}

impl std::fmt::Debug for Scripted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let left = self.steps.lock().map(|s| s.len()).unwrap_or(0);
        write!(f, "Scripted({}, {left} steps left)", self.id)
    }
}

/// One entry of a script file: a step, and when it plays.
#[derive(Clone, Debug, Deserialize)]
pub struct ScriptEntry {
    /// Play it only if the conversation so far contains this text (ignoring
    /// case).
    #[serde(default)]
    pub when: Option<String>,
    /// Play it only if the conversation so far does not contain this text.
    #[serde(default)]
    pub unless: Option<String>,
    /// The step.
    #[serde(flatten)]
    pub step: ScriptStep,
}

/// One step of a script file.
#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum ScriptStep {
    /// The model calls tools: `{"call": "name", "input": {…}}` or
    /// `{"calls": [{"call": …, "input": …}, …]}`.
    Calls {
        /// The calls.
        calls: Vec<ScriptCall>,
    },
    /// One call.
    Call(ScriptCall),
    /// The model says something and ends its turn: `{"say": "…"}`.
    Say {
        /// What it says.
        say: String,
    },
}

/// A tool call in a script file.
#[derive(Clone, Debug, Deserialize)]
pub struct ScriptCall {
    /// The tool.
    pub call: String,
    /// Its input (an empty object if left out).
    #[serde(default = "empty_object")]
    pub input: Value,
}

fn empty_object() -> Value {
    Value::Object(Default::default())
}

/// The text a step's condition is matched against: the system prompt and
/// every message, tool calls and results included, in lower case.
fn conversation_text(request: &Request) -> String {
    let mut text = request.system.to_lowercase();
    for message in &request.messages {
        for block in &message.content {
            text.push('\n');
            match block {
                Block::Text { text: t } => text.push_str(&t.to_lowercase()),
                Block::ToolUse { name, input, .. } => {
                    text.push_str(name);
                    text.push(' ');
                    text.push_str(&input.to_string().to_lowercase());
                }
                Block::ToolResult { content, .. } => text.push_str(&content.to_lowercase()),
            }
        }
    }
    text
}

fn reply(content: Vec<Block>, stop: Stop) -> Response {
    Response {
        content,
        stop,
        usage: Usage::default(),
        model: "scripted".into(),
    }
}

impl Scripted {
    /// A model named `id` with an empty script.
    pub fn new(id: &str) -> Scripted {
        Scripted {
            id: id.to_string(),
            steps: Mutex::new(VecDeque::new()),
            seen: Mutex::new(Vec::new()),
            used: AtomicUsize::new(0),
        }
    }

    /// A model that plays a script file: a JSON array of entries (see
    /// [`ScriptEntry`]).
    pub fn from_json(id: &str, script: &str) -> Result<Scripted, String> {
        let entries: Vec<ScriptEntry> =
            serde_json::from_str(script).map_err(|e| format!("the script is not valid: {e}"))?;
        let mut model = Scripted::new(id);
        for entry in entries {
            model = match entry.step {
                ScriptStep::Say { say } => model.says(&say),
                ScriptStep::Call(call) => model.calls_all(vec![(call.call, call.input)]),
                ScriptStep::Calls { calls } => {
                    model.calls_all(calls.into_iter().map(|c| (c.call, c.input)).collect())
                }
            };
            if entry.when.is_some() || entry.unless.is_some() {
                let mut steps = model.steps.lock().expect("the script's lock");
                if let Some(last) = steps.back_mut() {
                    last.when = entry.when;
                    last.unless = entry.unless;
                }
            }
        }
        Ok(model)
    }

    /// Next, the model says `text` and ends its turn.
    pub fn says(self, text: &str) -> Scripted {
        let text = text.to_string();
        self.then(move |_| Ok(reply(vec![Block::text(text.clone())], Stop::EndTurn)))
    }

    /// Next, the model calls `tool` with `input`.
    pub fn calls(self, tool: &str, input: Value) -> Scripted {
        self.calls_all(vec![(tool.to_string(), input)])
    }

    /// Next, the model calls several tools in one turn.
    pub fn calls_all(self, calls: Vec<(String, Value)>) -> Scripted {
        self.then(move |request| {
            let turn = request.messages.len();
            let content = calls
                .iter()
                .enumerate()
                .map(|(i, (name, input))| Block::ToolUse {
                    id: format!("call-{turn}-{i}"),
                    name: name.clone(),
                    input: input.clone(),
                })
                .collect();
            Ok(reply(content, Stop::ToolUse))
        })
    }

    /// Next, the call fails with `error`.
    pub fn fails(self, error: ModelError) -> Scripted {
        self.then(move |_| Err(error.clone()))
    }

    /// Next, the model answers with `f`.
    pub fn then(
        self,
        f: impl Fn(&Request) -> Result<Response, ModelError> + Send + Sync + 'static,
    ) -> Scripted {
        self.steps
            .lock()
            .expect("the script's lock")
            .push_back(Guarded {
                when: None,
                unless: None,
                step: Box::new(f),
            });
        self
    }

    /// The last step added plays only if the conversation contains `text`.
    pub fn when(self, text: &str) -> Scripted {
        if let Some(last) = self.steps.lock().expect("the script's lock").back_mut() {
            last.when = Some(text.to_string());
        }
        self
    }

    /// The last step added plays only if the conversation does not contain
    /// `text`.
    pub fn unless(self, text: &str) -> Scripted {
        if let Some(last) = self.steps.lock().expect("the script's lock").back_mut() {
            last.unless = Some(text.to_string());
        }
        self
    }

    /// The requests it has received, in order.
    pub fn requests(&self) -> Vec<Request> {
        self.seen.lock().expect("the requests' lock").clone()
    }

    /// Steps not played yet.
    pub fn left(&self) -> usize {
        self.steps.lock().expect("the script's lock").len()
    }

    /// Drops the first `n` steps: a resumed session's script goes on from
    /// where it was ([`Model::position`] says where).
    pub fn skip(self, n: usize) -> Scripted {
        {
            let mut steps = self.steps.lock().expect("the script's lock");
            for _ in 0..n.min(steps.len()) {
                steps.pop_front();
            }
        }
        self.used.fetch_add(n, Ordering::SeqCst);
        self
    }
}

impl Model for Scripted {
    fn id(&self) -> &str {
        &self.id
    }

    fn complete(&self, request: &Request) -> Result<Response, ModelError> {
        self.seen
            .lock()
            .expect("the requests' lock")
            .push(request.clone());
        let text = conversation_text(request);
        loop {
            let next = self.steps.lock().expect("the script's lock").pop_front();
            let Some(guarded) = next else {
                return Ok(reply(vec![Block::text("(end of script)")], Stop::EndTurn));
            };
            self.used.fetch_add(1, Ordering::SeqCst);
            if guarded.applies(&text) {
                return (guarded.step)(request);
            }
        }
    }

    fn position(&self) -> Option<usize> {
        Some(self.used.load(Ordering::SeqCst))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Message;
    use serde_json::json;

    fn request() -> Request {
        Request {
            system: String::new(),
            messages: vec![Message::user("hi")],
            tools: Vec::new(),
            max_tokens: 100,
        }
    }

    #[test]
    fn a_script_file_plays_in_order() {
        let model = Scripted::from_json(
            "s",
            r#"[{"call": "balance"}, {"calls": [{"call": "a", "input": {"x": 1}}, {"call": "b"}]}, {"say": "done"}]"#,
        )
        .unwrap();
        let first = model.complete(&request()).unwrap();
        assert_eq!(first.stop, Stop::ToolUse);
        assert!(matches!(&first.content[0], Block::ToolUse { name, .. } if name == "balance"));
        let second = model.complete(&request()).unwrap();
        assert_eq!(second.content.len(), 2);
        assert!(
            matches!(&second.content[0], Block::ToolUse { input, .. } if *input == json!({"x": 1}))
        );
        let third = model.complete(&request()).unwrap();
        assert_eq!(third.stop, Stop::EndTurn);
        assert_eq!(model.left(), 0);
        assert_eq!(model.requests().len(), 3);
        assert_eq!(model.position(), Some(3));
    }

    const CAREFUL: &str = r#"[
        {"when": "check the payee", "call": "lookup_payee", "input": {"name": "acme"}},
        {"unless": "lookup_payee", "say": "paid without checking"},
        {"call": "pay"},
        {"say": "paid"}
    ]"#;

    #[test]
    fn a_step_plays_only_when_its_condition_holds() {
        // Nothing says to check the payee: the first step is passed over.
        let model = Scripted::from_json("s", CAREFUL).unwrap();
        let first = model.complete(&request()).unwrap();
        assert_eq!(first.content, vec![Block::text("paid without checking")]);
        assert_eq!(model.position(), Some(2));

        // A skill that says to check it, in any case: the model checks, and
        // the step meant for a model that did not is passed over.
        let model = Scripted::from_json("s", CAREFUL).unwrap();
        let mut request = request();
        request.system = "Skill: Check the payee before paying.".into();
        let first = model.complete(&request).unwrap();
        assert!(matches!(&first.content[0], Block::ToolUse { name, .. } if name == "lookup_payee"));
        request.messages.push(Message {
            role: crate::model::Role::Assistant,
            content: first.content,
        });
        let second = model.complete(&request).unwrap();
        assert!(matches!(&second.content[0], Block::ToolUse { name, .. } if name == "pay"));
        assert_eq!(model.position(), Some(3));
    }

    #[test]
    fn a_resumed_script_goes_on_from_its_position() {
        let model = Scripted::from_json("s", CAREFUL).unwrap();
        model.complete(&request()).unwrap();
        let resumed = Scripted::from_json("s", CAREFUL)
            .unwrap()
            .skip(model.position().unwrap());
        let next = resumed.complete(&request()).unwrap();
        assert!(matches!(&next.content[0], Block::ToolUse { name, .. } if name == "pay"));
        assert_eq!(resumed.position(), Some(3));
    }
}
