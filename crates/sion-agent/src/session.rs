//! A session: the loop that asks the model, runs the tools it asks for under
//! the policy, and stops when the work is done, the turns run out, or a
//! person must decide.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::journal::{key, Entry, Journal, Prior};
use crate::model::{Block, Message, Model, ModelError, Request, Response, Role, Stop, Usage};
use crate::policy::{Decision, Policy, Standing};
use crate::tools::{Outcome, Toolbox, TOOLS_CALL, TOOLS_SEARCH};

/// The limits of a session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    /// Model turns allowed at first.
    pub turns: u32,
    /// Turns added after each turn in which a tool call succeeded.
    pub turns_per_progress: u32,
    /// The most turns in all, whatever the progress.
    pub max_turns: u32,
    /// Tokens per model call.
    pub max_tokens: u32,
    /// Failures of one identical call after which it is blocked.
    pub failures_to_block: u32,
    /// Compact the conversation when it has more messages than this…
    pub compact_messages: usize,
    /// …or more characters than this.
    pub compact_chars: usize,
    /// Messages kept as they are at the end when compacting.
    pub keep_last: usize,
    /// Seconds a model that failed is not asked again.
    pub cooldown: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            turns: 12,
            turns_per_progress: 4,
            max_turns: 32,
            max_tokens: 4_096,
            failures_to_block: 2,
            compact_messages: 36,
            compact_chars: 120_000,
            keep_last: 12,
            cooldown: 30,
        }
    }
}

/// Why a run stopped.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "ended", rename_all = "snake_case")]
pub enum Ended {
    /// The model finished its work.
    Done {
        /// Its last words.
        text: String,
    },
    /// A call waits for a person's approval; approve or decline it, then run
    /// again.
    NeedsApproval {
        /// The call's key.
        key: String,
        /// The tool.
        tool: String,
        /// Its input.
        input: Value,
        /// Why it needs approval.
        reason: String,
    },
    /// The turns ran out.
    OutOfTurns,
    /// The model declined the work.
    Refused,
    /// No model could answer.
    ModelsFailed {
        /// The last error.
        error: String,
    },
    /// The kill switch is on.
    Stopped,
}

/// A call waiting for approval.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pending {
    /// The tool use it answers.
    pub use_id: String,
    /// The key of the call to approve (inside `tools_call`, the inner
    /// call's).
    pub key: String,
    /// The tool the model called.
    pub tool: String,
    /// Its input.
    pub input: Value,
}

/// Everything a session needs to stop and resume. It serialises.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    /// The session's identifier, part of every call's key.
    pub id: String,
    /// The conversation.
    pub messages: Vec<Message>,
    /// Turns left.
    pub turns_left: u32,
    /// Turns taken.
    pub turns_taken: u32,
    /// Approvals and taint.
    pub standing: Standing,
    /// The calls that changed something.
    pub journal: Journal,
    /// Failures of each call, by key.
    pub failures: BTreeMap<String, u32>,
    /// Tokens used.
    pub usage: Usage,
    /// Until when each model is not asked (seconds), by its index.
    pub cooling: BTreeMap<usize, u64>,
    /// The call waiting for approval, if any.
    pub pending: Option<Pending>,
    /// How many times the conversation was compacted.
    pub compactions: u32,
}

impl State {
    /// A new session `id` that starts with the person's `task`.
    pub fn start(id: &str, task: &str, limits: &Limits) -> State {
        State {
            id: id.to_string(),
            messages: vec![Message::user(task)],
            turns_left: limits.turns,
            ..State::default()
        }
    }

    /// Adds a message from the person, to continue a session that ended.
    pub fn tell(&mut self, text: &str, limits: &Limits) {
        self.messages.push(Message::user(text));
        self.turns_left = self.turns_left.max(
            limits
                .turns
                .min(limits.max_turns.saturating_sub(self.turns_taken)),
        );
    }
}

/// What a session runs with, besides its state: models, tools, rules, a
/// clock and a kill switch.
pub struct Runtime<'a> {
    /// The models, in order of preference.
    pub models: Vec<&'a dyn Model>,
    /// The tools.
    pub tools: &'a Toolbox,
    /// The rules.
    pub policy: Policy,
    /// The limits.
    pub limits: Limits,
    /// The system prompt.
    pub system: String,
    /// The clock, in seconds.
    pub now: &'a (dyn Fn() -> u64 + Sync),
    /// The kill switch: when it returns true, the run stops before its next
    /// model call or tool call.
    pub stopped: &'a (dyn Fn() -> bool + Sync),
}

impl std::fmt::Debug for Runtime<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Runtime")
            .field(
                "models",
                &self.models.iter().map(|m| m.id()).collect::<Vec<_>>(),
            )
            .field("tools", &self.tools)
            .field("policy", &self.policy)
            .field("limits", &self.limits)
            .finish()
    }
}

enum Called {
    Ran(Outcome),
    /// The call (inside `tools_call`, the inner one) needs approval.
    Wait {
        key: String,
        tool: String,
        input: Value,
        reason: String,
    },
}

/// What the checks say about a call before it runs.
enum Checked {
    /// Answered without running: from the journal, refused, blocked.
    Answered(Outcome),
    /// Needs a person's approval.
    Wait {
        key: String,
        tool: String,
        input: Value,
        reason: String,
    },
    /// May run: the tool (inside `tools_call`, the inner one), its input,
    /// the call's key and effect, and whether it is journaled.
    Ready(Ready),
}

/// A call that passed its checks.
struct Ready {
    tool: String,
    input: Value,
    key: String,
    effect: crate::tools::Effect,
    journaled: bool,
}

/// What a tool use waiting for approval says to the model meanwhile.
const WAITING: &str = "waiting for a person's approval";

/// What precedes an outcome the journal answers a repeated call with.
pub const REPLAYED: &str = "(Not done again: an identical call earlier in this session already \
     did this. To do it again on purpose, change its input. It returned:)";

impl Runtime<'_> {
    /// Runs the session until it ends or must wait.
    pub fn run(&self, state: &mut State) -> Ended {
        loop {
            if (self.stopped)() {
                return Ended::Stopped;
            }
            if let Some(pending) = state.pending.clone() {
                match self.call(state, &pending.tool, &pending.input) {
                    Called::Wait {
                        key,
                        tool,
                        input,
                        reason,
                    } => {
                        return Ended::NeedsApproval {
                            key,
                            tool,
                            input,
                            reason,
                        }
                    }
                    Called::Ran(outcome) => {
                        state.pending = None;
                        replace_result(state, &pending.use_id, &outcome);
                    }
                }
            }
            if state.turns_left == 0 {
                return Ended::OutOfTurns;
            }
            self.compact(state);
            let response = match self.ask(state) {
                Ok(response) => response,
                Err(error) => {
                    return Ended::ModelsFailed {
                        error: error.to_string(),
                    }
                }
            };
            state.turns_left -= 1;
            state.turns_taken += 1;
            state.usage.input += response.usage.input;
            state.usage.output += response.usage.output;
            let message = Message {
                role: Role::Assistant,
                content: response.content,
            };
            let calls: Vec<(String, String, Value)> = message
                .tool_uses()
                .map(|(id, name, input)| (id.to_string(), name.to_string(), input.clone()))
                .collect();
            let text = message.text();
            state.messages.push(message);
            if calls.is_empty() {
                match response.stop {
                    Stop::Refusal => return Ended::Refused,
                    Stop::PauseTurn => continue,
                    Stop::MaxTokens => {
                        state.messages.push(Message::user(
                            "Your answer was cut off at the token limit. Continue from where it stopped.",
                        ));
                        continue;
                    }
                    _ => return Ended::Done { text },
                }
            }
            if let Some(ended) = self.run_calls(state, &calls) {
                return ended;
            }
        }
    }

    /// Runs one turn's calls in order, stopping at the first that needs
    /// approval.
    fn run_calls(&self, state: &mut State, calls: &[(String, String, Value)]) -> Option<Ended> {
        let mut results = Vec::new();
        let mut progress = false;
        let mut index = 0;
        while index < calls.len() {
            let (use_id, name, input) = &calls[index];
            // The calls of a batching tool that follow this one in the turn,
            // if they too may run, run with it: each passed the policy by
            // itself, and none was decided after seeing another's result.
            if let Checked::Ready(first) = self.check(state, name, input) {
                let tool = self.tools.get(&first.tool).expect("checked");
                if tool.batches() && name != TOOLS_CALL {
                    let mut group = vec![(index, first)];
                    let mut next = index + 1;
                    while next < calls.len() && calls[next].1 == *name {
                        match self.check(state, name, &calls[next].2) {
                            // An identical call waits for the first's record.
                            Checked::Ready(ready)
                                if group.iter().all(|(_, g)| g.key != ready.key) =>
                            {
                                group.push((next, ready));
                                next += 1;
                            }
                            _ => break,
                        }
                    }
                    let inputs: Vec<(&Value, &str)> = group
                        .iter()
                        .map(|(_, r)| (&r.input, r.key.as_str()))
                        .collect();
                    let mut outcomes = tool.call_batch(&inputs);
                    outcomes.resize_with(group.len(), || {
                        Outcome::uncertain("the tool gave no outcome for this call")
                    });
                    for ((at, ready), outcome) in group.iter().zip(outcomes) {
                        self.finish(state, ready, &outcome);
                        progress |= !outcome.is_error;
                        results.push(result_block(&calls[*at].0, &outcome));
                    }
                    index = next;
                    continue;
                }
            }
            index += 1;
            match self.call(state, name, input) {
                Called::Ran(outcome) => {
                    progress |= !outcome.is_error;
                    results.push(result_block(use_id, &outcome));
                }
                Called::Wait {
                    key,
                    tool,
                    input: inner,
                    reason,
                } => {
                    results.push(result_block(use_id, &Outcome::ok(WAITING)));
                    for (later, _, _) in &calls[index..] {
                        results.push(result_block(
                            later,
                            &Outcome::error("not run: an earlier call in this turn waited for approval; call it again if it is still needed"),
                        ));
                    }
                    state.messages.push(Message {
                        role: Role::User,
                        content: results,
                    });
                    state.pending = Some(Pending {
                        use_id: use_id.clone(),
                        key: key.clone(),
                        tool: name.clone(),
                        input: input.clone(),
                    });
                    return Some(Ended::NeedsApproval {
                        key,
                        tool,
                        input: inner,
                        reason,
                    });
                }
            }
        }
        if progress {
            let room = self.limits.max_turns.saturating_sub(state.turns_taken);
            state.turns_left = (state.turns_left + self.limits.turns_per_progress).min(room);
        }
        state.messages.push(Message {
            role: Role::User,
            content: results,
        });
        None
    }

    /// Approves the call waiting in `state`: the next run makes it.
    pub fn approve(&self, state: &mut State) -> bool {
        match &state.pending {
            Some(pending) => {
                state
                    .standing
                    .approve(&self.policy, &pending.key, (self.now)());
                true
            }
            None => false,
        }
    }

    /// Declines the call waiting in `state`: the model is told why, and the
    /// next run continues without it.
    pub fn decline(&self, state: &mut State, why: &str) -> bool {
        match state.pending.take() {
            Some(pending) => {
                replace_result(
                    state,
                    &pending.use_id,
                    &Outcome::error(format!("a person declined this call: {why}")),
                );
                true
            }
            None => false,
        }
    }

    fn ask(&self, state: &mut State) -> Result<Response, ModelError> {
        let request = Request {
            system: self.system.clone(),
            messages: state.messages.clone(),
            tools: self.tools.shown(),
            max_tokens: self.limits.max_tokens,
        };
        let now = (self.now)();
        let mut last = ModelError::Refused("no model is configured".into());
        let mut asked = false;
        for (index, model) in self.models.iter().enumerate() {
            if state.cooling.get(&index).is_some_and(|until| now < *until) {
                continue;
            }
            asked = true;
            match model.complete(&request) {
                Ok(response) => return Ok(response),
                Err(error) if error.transient() => {
                    state
                        .cooling
                        .insert(index, now.saturating_add(self.limits.cooldown));
                    last = error;
                }
                Err(error) => return Err(error),
            }
        }
        if !asked {
            last = ModelError::Busy("every model is cooling down after a failure".into());
        }
        Err(last)
    }

    fn call(&self, state: &mut State, name: &str, input: &Value) -> Called {
        match self.check(state, name, input) {
            Checked::Answered(outcome) => Called::Ran(outcome),
            Checked::Wait {
                key,
                tool,
                input,
                reason,
            } => Called::Wait {
                key,
                tool,
                input,
                reason,
            },
            Checked::Ready(ready) => {
                let tool = self.tools.get(&ready.tool).expect("checked");
                let outcome = if state.standing.tainted((self.now)()) {
                    tool.call_tainted(&ready.input, &ready.key)
                } else {
                    tool.call(&ready.input, &ready.key)
                };
                self.finish(state, &ready, &outcome);
                Called::Ran(outcome)
            }
        }
    }

    /// Everything that decides whether a call runs, without running it:
    /// the meta-tools, the journal, the policy and the kill switch. It
    /// changes nothing, so a call may be checked again.
    fn check(&self, state: &State, name: &str, input: &Value) -> Checked {
        // The meta-tools of a large toolbox.
        if name == TOOLS_SEARCH {
            let query = input["query"].as_str().unwrap_or_default();
            let found = self.tools.search(query);
            return Checked::Answered(Outcome::ok(json!(found).to_string()));
        }
        if name == TOOLS_CALL {
            let inner = input["name"].as_str().unwrap_or_default().to_string();
            if inner == TOOLS_CALL || inner == TOOLS_SEARCH {
                return Checked::Answered(Outcome::error("tools_call calls a tool, not itself"));
            }
            let inner_input = input.get("input").cloned().unwrap_or_else(|| json!({}));
            return self.check(state, &inner, &inner_input);
        }
        let call_key = key(&state.id, name, input);
        let Some(tool) = self.tools.get(name) else {
            return Checked::Answered(Outcome::error(format!("there is no tool named {name}")));
        };
        if state.failures.get(&call_key).copied().unwrap_or(0) >= self.limits.failures_to_block {
            return Checked::Answered(Outcome::error(
                "blocked: this exact call has failed repeatedly; change its input or the approach",
            ));
        }
        let journaled = tool.journaled();
        match state.journal.prior(&call_key) {
            Prior::Done(outcome) if journaled => {
                // Said, so that the model does not take a recorded outcome
                // for news.
                return Checked::Answered(Outcome {
                    content: format!("{REPLAYED}\n{}", outcome.content),
                    ..outcome
                });
            }
            Prior::Unknown if journaled => {
                return Checked::Answered(Outcome::uncertain(
                    "not repeated: an identical earlier call may or may not have taken effect; \
                     check its result before trying again",
                ))
            }
            _ => {}
        }
        let now = (self.now)();
        // A call can do at most what its tool declares.
        let effect = tool.effect_of(input).min(tool.effect());
        match self
            .policy
            .decide(&state.standing, name, effect, &call_key, now)
        {
            Decision::Deny(reason) => {
                return Checked::Answered(Outcome::error(format!("refused by policy: {reason}")))
            }
            Decision::Ask(reason) => {
                return Checked::Wait {
                    key: call_key,
                    tool: name.to_string(),
                    input: input.clone(),
                    reason,
                }
            }
            Decision::Run => {}
        }
        if (self.stopped)() {
            return Checked::Answered(Outcome::error("not run: the kill switch is on"));
        }
        Checked::Ready(Ready {
            tool: name.to_string(),
            input: input.clone(),
            key: call_key,
            effect,
            journaled,
        })
    }

    /// Records what a call that ran did: its taint, its failure, and, if it
    /// is journaled, its outcome.
    fn finish(&self, state: &mut State, ready: &Ready, outcome: &Outcome) {
        let now = (self.now)();
        if outcome.untrusted {
            state.standing.taint(&self.policy, now);
        }
        if outcome.is_error && outcome.certain {
            *state.failures.entry(ready.key.clone()).or_default() += 1;
        }
        if ready.journaled {
            state.journal.record(
                &ready.key,
                Entry {
                    tool: ready.tool.clone(),
                    input: ready.input.clone(),
                    effect: ready.effect,
                    outcome: outcome.clone(),
                    at: now,
                },
            );
        }
    }

    /// Replaces the middle of a long conversation with a summary of it,
    /// keeping the task and the latest messages.
    fn compact(&self, state: &mut State) {
        let chars: usize = state.messages.iter().map(Message::chars).sum();
        if state.messages.len() <= self.limits.compact_messages
            && chars <= self.limits.compact_chars
        {
            return;
        }
        let keep = self
            .limits
            .keep_last
            .min(state.messages.len().saturating_sub(2));
        // Start the kept tail on a message that is not a tool result, so that
        // no result is kept without the call it answers.
        let mut tail = state.messages.len() - keep;
        while tail > 1 && is_results(&state.messages[tail]) {
            tail -= 1;
        }
        if tail <= 1 {
            return;
        }
        let summary = summarize(&state.messages[1..tail]);
        let mut messages = vec![state.messages[0].clone()];
        messages.push(Message {
            role: Role::Assistant,
            content: vec![Block::text(summary)],
        });
        if state.messages[tail].role == Role::Assistant {
            messages.push(Message::user("Continue."));
        }
        messages.extend_from_slice(&state.messages[tail..]);
        state.messages = messages;
        state.compactions += 1;
    }
}

fn is_results(message: &Message) -> bool {
    message
        .content
        .iter()
        .any(|b| matches!(b, Block::ToolResult { .. }))
}

fn result_block(use_id: &str, outcome: &Outcome) -> Block {
    Block::ToolResult {
        id: use_id.to_string(),
        content: outcome.content.clone(),
        is_error: outcome.is_error,
    }
}

fn replace_result(state: &mut State, use_id: &str, outcome: &Outcome) {
    for message in state.messages.iter_mut().rev() {
        for block in message.content.iter_mut() {
            if matches!(block, Block::ToolResult { id, .. } if id == use_id) {
                *block = result_block(use_id, outcome);
                return;
            }
        }
    }
}

/// An extractive summary of messages: what the model said and called, and
/// what came back, one shortened line each.
pub fn summarize(messages: &[Message]) -> String {
    let mut lines = vec![format!(
        "[Summary of {} earlier messages, compacted to save context]",
        messages.len()
    )];
    for message in messages {
        for block in &message.content {
            lines.push(match block {
                Block::Text { text } if message.role == Role::Assistant => {
                    format!("- I said: {}", shorten(text, 240))
                }
                Block::Text { text } => format!("- I was told: {}", shorten(text, 240)),
                Block::ToolUse { name, input, .. } => {
                    format!(
                        "- I called {name} with {}",
                        shorten(&input.to_string(), 160)
                    )
                }
                Block::ToolResult {
                    content, is_error, ..
                } => format!(
                    "  → {}{}",
                    if *is_error { "error: " } else { "" },
                    shorten(content, 200)
                ),
            });
        }
    }
    lines.join("\n")
}

fn shorten(text: &str, max: usize) -> String {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= max {
        one_line
    } else {
        let cut: String = one_line.chars().take(max).collect();
        format!("{cut}…")
    }
}
