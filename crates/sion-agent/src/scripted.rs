//! A model that plays a script: for tests, demos and replaying a session.
//! It makes no network call.

use std::collections::VecDeque;
use std::sync::Mutex;

use serde::Deserialize;
use serde_json::Value;

use crate::model::{Block, Model, ModelError, Request, Response, Stop, Usage};

type Step = Box<dyn Fn(&Request) -> Result<Response, ModelError> + Send + Sync>;

/// A model that answers each call with the next step of its script.
pub struct Scripted {
    id: String,
    steps: Mutex<VecDeque<Step>>,
    seen: Mutex<Vec<Request>>,
}

impl std::fmt::Debug for Scripted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let left = self.steps.lock().map(|s| s.len()).unwrap_or(0);
        write!(f, "Scripted({}, {left} steps left)", self.id)
    }
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
        }
    }

    /// A model that plays a script file: a JSON array of steps (see
    /// [`ScriptStep`]).
    pub fn from_json(id: &str, script: &str) -> Result<Scripted, String> {
        let steps: Vec<ScriptStep> =
            serde_json::from_str(script).map_err(|e| format!("the script is not valid: {e}"))?;
        let mut model = Scripted::new(id);
        for step in steps {
            model = match step {
                ScriptStep::Say { say } => model.says(&say),
                ScriptStep::Call(call) => model.calls_all(vec![(call.call, call.input)]),
                ScriptStep::Calls { calls } => {
                    model.calls_all(calls.into_iter().map(|c| (c.call, c.input)).collect())
                }
            };
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
            .push_back(Box::new(f));
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
        let step = self.steps.lock().expect("the script's lock").pop_front();
        match step {
            Some(step) => step(request),
            None => Ok(reply(vec![Block::text("(end of script)")], Stop::EndTurn)),
        }
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
    }
}
