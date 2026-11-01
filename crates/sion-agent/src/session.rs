//! A session: the loop that asks the model, runs the tools it asks for under
//! the policy, and stops when the work is done, the turns run out, or a
//! person must decide.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::journal::{key, Entry, Journal, Prior};
use crate::model::{
    image_digest, images_of, Block, Delta, Image, Message, Model, ModelError, Picture, Request,
    Response, Role, Stop, ToolSpec, Usage,
};
use crate::policy::{Decision, Policy, Standing};
use crate::quarantine::{self, Held, QUARANTINE_READ};
use crate::tools::{Dedup, Effect, Outcome, Toolbox, TOOLS_CALL, TOOLS_SEARCH};
use crate::verify::{self, Attempts, Verdict, CANNOT_COMPLETE};

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
    /// Times one call may be made with the same result before the model is
    /// told it is going round in circles; two more, and the run stops
    /// ([`Ended::Stuck`]). Two calls that alternate with the same results
    /// count each round as one.
    #[serde(default = "default_stuck_after")]
    pub stuck_after: u32,
    /// Tokens a run may use, its subagents' included, if it has a budget:
    /// each model call is checked against what is left before it is sent
    /// (what it is sent, and the most it may answer), and the run ends
    /// ([`Ended::OutOfBudget`]) rather than go over.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_tokens: Option<u64>,
    /// Tokens of the budget kept for a last answer without tools, in which
    /// the model says what it did and what remains.
    #[serde(default = "default_reserve_tokens")]
    pub reserve_tokens: u64,
    /// Checks of the model's answers that may fail before a run with a
    /// verifier ends unverified ([`Runtime::verify`], [`Ended::Unverified`]).
    #[serde(default = "default_verify_attempts")]
    pub verify_attempts: u32,
}

fn default_stuck_after() -> u32 {
    3
}

fn default_verify_attempts() -> u32 {
    3
}

fn default_reserve_tokens() -> u64 {
    2_000
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
            stuck_after: default_stuck_after(),
            budget_tokens: None,
            reserve_tokens: default_reserve_tokens(),
            verify_attempts: default_verify_attempts(),
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
    /// The model went round in circles: the same calls with the same
    /// results, after it was told.
    Stuck {
        /// What it repeated.
        why: String,
    },
    /// The run's token budget ran out ([`Limits::budget_tokens`]).
    OutOfBudget {
        /// What the model said in its last answer, within the reserve,
        /// if there was room for one.
        text: String,
    },
    /// The checks of the model's answers failed, as many as
    /// [`Limits::verify_attempts`] allows (or the turns ran out after one):
    /// the run ends with the best answer it gave, not done.
    Unverified {
        /// The best answer: the one whose check scored highest, the later
        /// of two that scored the same.
        text: String,
        /// Which answer it was, from 1.
        attempt: u32,
        /// The last line of what the last check reported.
        why: String,
    },
    /// The model said that it cannot complete the task ([`CANNOT_COMPLETE`]).
    CannotComplete {
        /// Why.
        why: String,
    },
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
    /// What the call can do, as its tool declared it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect: Option<Effect>,
}

impl Pending {
    /// The approval request's id, as a person is shown it and may give it
    /// back (`/approve 3f2a9c1e`): the first eight digits of the call's
    /// key, the same for as long as the call waits.
    pub fn request(&self) -> &str {
        &self.key[..self.key.len().min(8)]
    }
}

/// What approving a call allows besides it ([`Runtime::approve_as`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    /// That call only.
    Once,
    /// That call, made again with the same input, for the rest of the
    /// session.
    Call,
    /// The calls of its tool this rule matches, for the rest of the
    /// session (as a skill's `allowed-tools` writes it: `shell(git
    /// status:*)`, `fetch_url(domain:example.org)`).
    Rule(crate::permissions::ToolRule),
}

/// A person's decision on a call, as the session keeps it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decided {
    /// The approval request's id ([`Pending::request`]).
    pub request: String,
    /// The tool.
    pub tool: String,
    /// Whether it was approved.
    pub approved: bool,
    /// Who decided, as the channel names them.
    pub by: String,
    /// When (seconds).
    pub at: u64,
    /// What else it allowed, in words, if anything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed: Option<String>,
    /// Why it was declined.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// The deciding person's wallet's signature over the decision, if they
    /// signed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<DecisionSignature>,
}

/// A wallet's signature over a decision: over the session, the call's key,
/// the digest of its input and what was decided, as the CLI encodes them
/// (Borsh, then a domain-tagged hash). The keys and signatures are kept
/// encoded, so that this crate needs none of their cryptography.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionSignature {
    /// The wallet that signed, as it was named.
    pub wallet: String,
    /// The account its keys are (hex).
    pub account: String,
    /// The call's key, all of it.
    pub key: String,
    /// The digest of the call's input (hex).
    pub input: String,
    /// The account's authorization policy, Borsh-encoded (hex).
    pub policy: String,
    /// The signatures, Borsh-encoded (hex).
    pub authorization: String,
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
    /// The bytes (base64) of the last pictures tools returned, by their
    /// digests: at most [`MAX_IMAGES_KEPT`].
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub images: BTreeMap<String, String>,
    /// The steps of its runs, for its trace: the last [`MAX_STEPS`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<Step>,
    /// The decisions people made on its calls, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decisions: Vec<Decided>,
    /// The last calls made and what they gave, to tell when the model goes
    /// round in circles: at most [`RECENT_KEPT`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent: Vec<Made>,
    /// Tokens the calls' own models used (subagents at work).
    #[serde(default)]
    pub delegated: Usage,
    /// What tools returned from outside and was held out of the
    /// conversation, in quarantine ([`crate::quarantine`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub held: Vec<Held>,
    /// The strings read from content held in quarantine: an external call
    /// or a payment whose input carries one waits for a person.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub outside_values: BTreeSet<String>,
    /// What the session did that lasts, for the summaries of a long
    /// conversation ([`lasting`]): its calls that changed something or
    /// reached outside and did not fail, oldest first, the last
    /// [`LASTING_KEPT`] of them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub done: Vec<Done>,
    /// How many calls of each tool were let go from [`State::done`].
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub done_before: BTreeMap<String, u64>,
    /// The latest outcome of each thing the session read or changed, by
    /// what its tool names it ([`crate::tools::Tool::resource`]), kept
    /// apart from the conversation like [`State::done`]: the
    /// [`LATEST_KEPT`] most recent.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub latest: BTreeMap<String, Latest>,
    /// The checks of the model's answers since the person last spoke
    /// ([`Runtime::verify`]).
    #[serde(default, skip_serializing_if = "Attempts::is_empty")]
    pub attempts: Attempts,
}

/// The most recent calls a session keeps to tell when it goes round in
/// circles.
pub const RECENT_KEPT: usize = 16;

/// The most pictures a session keeps the bytes of: the last ones. The
/// conversation keeps every picture's digest.
pub const MAX_IMAGES_KEPT: usize = 6;

/// The most steps a session keeps for its trace: the last ones.
pub const MAX_STEPS: usize = 2_000;

/// One step of a run, for its trace ([`crate::trace::otel`]): a model
/// asked, or a call answered.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    /// `model` or `tool`.
    pub kind: String,
    /// The model that answered, or the tool called.
    pub name: String,
    /// When it began, in milliseconds since 1970, by the machine's clock.
    pub start_ms: u64,
    /// How long it took, in milliseconds.
    pub ms: u64,
    /// A model's tokens: in, and out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<(u64, u64)>,
    /// Of a model's input, the tokens read from its provider's cache and
    /// those written to it, if it reported any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached: Option<(u64, u64)>,
    /// A call's key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// A call's effect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect: Option<Effect>,
    /// How a call ended: `ok`, `error`, `unknown` (it may or may not have
    /// taken effect), `answered` (by the journal or the checks, without
    /// running) or `waiting` (for a person's approval).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    /// Whether the session had read content from outside when the step
    /// began.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub tainted: bool,
}

/// Milliseconds since 1970, by the machine's clock: for traces only, never
/// for what a run decides.
fn wall_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

impl State {
    /// A new session `id` that starts with the person's `task`.
    pub fn start(id: &str, task: &str, limits: &Limits) -> State {
        State::start_with(id, task, &[], limits)
    }

    /// [`State::start`], the task with the pictures the person sent.
    pub fn start_with(id: &str, task: &str, pictures: &[Picture], limits: &Limits) -> State {
        let mut state = State {
            id: id.to_string(),
            messages: vec![Message::user(task)],
            turns_left: limits.turns,
            ..State::default()
        };
        state.show(pictures);
        state
    }

    /// Whether a person may allow more than the call that waits: the same
    /// call again, or the calls a rule matches, for the rest of the session
    /// (`--always`). Not for a payment, which is approved one at a time;
    /// not while the session is tainted at `now`, nor once it holds
    /// confidential content, since an allowance would not hold; and not for
    /// a call whose argument comes from content held in quarantine, which a
    /// person approves each time it is used.
    pub fn may_allow_more(&self, now: u64) -> bool {
        self.pending.as_ref().is_some_and(|pending| {
            pending.effect.is_some_and(|effect| effect < Effect::Spend)
                && self.standing.open(now)
                && quarantine::carried(&pending.input, &self.outside_values).is_none()
        })
    }

    /// Adds `pictures` to the person's last message, their bytes kept
    /// with the last pictures'.
    fn show(&mut self, pictures: &[Picture]) {
        if pictures.is_empty() {
            return;
        }
        let Some(last) = self.messages.last_mut().filter(|m| m.role == Role::User) else {
            return;
        };
        for picture in pictures {
            let image = picture.image();
            self.images
                .insert(image.digest.clone(), picture.base64.clone());
            last.content.push(Block::Image { image });
        }
        keep_last_images(self);
    }

    /// Adds what the person said while a run goes on: to the last message
    /// if it is theirs (a task, or tool results, which the model answers
    /// next), so that the conversation still takes turns.
    pub fn steer(&mut self, text: &str) {
        let said = format!("(The person said, while you worked:) {text}");
        match self.messages.last_mut() {
            Some(last) if last.role == Role::User => last.content.push(Block::text(said)),
            _ => self.messages.push(Message::user(said)),
        }
    }

    /// [`State::tell`], with the pictures the person sent.
    pub fn tell_with(&mut self, text: &str, pictures: &[Picture], limits: &Limits) {
        self.tell(text, limits);
        self.show(pictures);
    }

    /// Adds a message from the person, to continue a session that ended.
    pub fn tell(&mut self, text: &str, limits: &Limits) {
        // After a run stopped before the model answered, the person speaks
        // again: one message of theirs, so that the conversation still
        // takes turns.
        match self.messages.last_mut() {
            Some(last) if last.role == Role::User => last.content.push(Block::text(text)),
            _ => self.messages.push(Message::user(text)),
        }
        self.turns_left = self.turns_left.max(
            limits
                .turns
                .min(limits.max_turns.saturating_sub(self.turns_taken)),
        );
        // What they said now is checked afresh.
        self.attempts = Attempts::default();
    }
}

/// What a run shows as it goes, to whoever watches it ([`Runtime::watch`]):
/// what lets a messenger show that the agent is typing, and its reply as it
/// is written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Seen {
    /// A run of the session begins: the session's id.
    Began(String),
    /// A model is asked for the next step.
    Asking,
    /// More of the text the model writes, as [`Delta::Text`].
    Text(String),
    /// The model began a call of this tool.
    Calling(String),
    /// The model's answer is taken; why it stopped.
    Answered(Stop),
    /// The answer being written is dropped: the model failed partway, and
    /// the next model (if any) is asked instead.
    Dropped,
    /// A tool runs.
    Running(String),
    /// A call waits for a person's approval.
    Waiting(String),
    /// The model's answer is being checked ([`Runtime::verify`]).
    Checking,
}

/// What the runtime tells a model whose answer was cut off at its token
/// limit, so that it goes on: its next answer continues the last.
pub const CONTINUE: &str =
    "Your answer was cut off at the token limit. Continue from where it stopped.";

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
    /// Who watches the run, if anyone: told what it does as it goes, and,
    /// while a model writes, its text (the models are asked to stream).
    pub watch: Option<&'a (dyn Fn(&Seen) + Sync)>,
    /// What the person said while the run went on, if anything: asked
    /// before each model call, and what it gives is added to the
    /// conversation ([`State::steer`]), so that the run takes it in.
    pub steer: Option<&'a (dyn Fn() -> Vec<String> + Sync)>,
    /// What keeps the session on disk while it runs, if anything: given
    /// the state before a journaled call runs, after each note it takes and
    /// after it ends, so that a crash loses no record of what was done.
    pub checkpoint: Option<&'a (dyn Fn(&State) + Sync)>,
    /// What checks that the task is done, if anything: given the session
    /// and the model's answer each time the model says it has finished
    /// ([`crate::verify`]). With it, the model may also say that it cannot
    /// complete the task ([`CANNOT_COMPLETE`]).
    pub verify: Option<Check<'a>>,
}

/// What checks a model's answer ([`Runtime::verify`]): given the session and
/// the answer, it says whether the task is done.
pub type Check<'a> = &'a (dyn Fn(&State, &str) -> Verdict + Sync);

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
        effect: Effect,
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
        effect: Effect,
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
    /// How its tool keeps it from happening twice.
    dedup: Dedup,
    /// Whether an earlier identical call's outcome is unknown, and the
    /// tool keeps a repeat from happening twice: it is made again.
    retried: bool,
    /// The skill and rule that pre-approved the call, when the autonomy
    /// level would have asked.
    pre_approved: Option<String>,
}

/// An outcome as the model sees it: a pre-approved call's says so, and so
/// does a call made again after its first outcome was lost.
fn marked(ready: &Ready, mut outcome: Outcome) -> Outcome {
    if let Some(labelled) = crate::imitation::label(&outcome.content) {
        outcome.content = labelled;
    }
    if let Some(by) = &ready.pre_approved {
        outcome.content = format!("[run without asking: {by}]\n{}", outcome.content);
    }
    if ready.retried {
        outcome.content = format!(
            "[made again: the first identical call's outcome was lost, and this tool keeps \
             it from happening twice]\n{}",
            outcome.content
        );
    }
    outcome
}

/// What a tool use waiting for approval says to the model meanwhile.
const WAITING: &str = "waiting for a person's approval";

/// What the model is told when the run's token budget is nearly spent: its
/// last answer.
pub const BUDGET_SPENT: &str = "[The token budget for this task is nearly spent. Do not call \
     tools now. Say what you did, what you found, and what remains to be done.]";

/// What a run used, when it began: to count what it uses against its
/// budget.
#[derive(Clone, Copy)]
struct RunStart {
    usage: Usage,
    delegated: Usage,
}

impl RunStart {
    fn of(state: &State) -> RunStart {
        RunStart {
            usage: state.usage,
            delegated: state.delegated,
        }
    }

    /// Tokens used since, the subagents' included.
    fn spent(&self, state: &State) -> u64 {
        state.usage.total().saturating_sub(self.usage.total())
            + state
                .delegated
                .total()
                .saturating_sub(self.delegated.total())
    }
}

/// A call a session made lately, to tell when it goes round in circles.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Made {
    /// The call's key.
    pub key: String,
    /// Its tool.
    pub tool: String,
    /// A digest of what it gave.
    pub gave: String,
}

/// Calls going round in circles.
struct Circling {
    /// How many times in a row (rounds, for two that take turns).
    count: usize,
    /// The tool repeated, or the two that take turns.
    tools: Vec<String>,
    /// Whether it is two more than the limit: the run stops.
    stop: bool,
}

/// Whether the last calls go round in circles: the same call with the same
/// result `after` times or more in a row, or two that take turns with the
/// same results for `after` rounds or more.
fn circling(recent: &[Made], after: u32) -> Option<Circling> {
    let n = recent.len();
    if n == 0 || after == 0 {
        return None;
    }
    let last = &recent[n - 1];
    let same = recent.iter().rev().take_while(|r| *r == last).count();
    let rounds = if n >= 2 && recent[n - 1] != recent[n - 2] {
        let (a, b) = (&recent[n - 1], &recent[n - 2]);
        recent
            .iter()
            .rev()
            .enumerate()
            .take_while(|(i, r)| if i % 2 == 0 { *r == a } else { *r == b })
            .count()
            / 2
    } else {
        0
    };
    let after = after as usize;
    let (count, tools) = if same >= rounds {
        (same, vec![last.tool.clone()])
    } else {
        (rounds, vec![recent[n - 2].tool.clone(), last.tool.clone()])
    };
    (count >= after).then(|| Circling {
        count,
        tools,
        stop: count >= after + 2,
    })
}

/// Adds the runtime's `note` to the model to what it is told next: to the
/// last call's result, where there is one, so that it is not taken for
/// the person's words; otherwise as text of its own.
fn note_to_model(blocks: &mut Vec<Block>, note: &str) {
    match blocks.last_mut() {
        Some(Block::ToolResult { content, .. }) => {
            content.push('\n');
            content.push_str(note);
        }
        _ => blocks.push(Block::text(note)),
    }
}

/// A digest of what a call gave, to tell when it gives the same again.
fn result_digest(outcome: &Outcome) -> String {
    sion_core::hash::hash_parts("sion-agent/recent/v1", &[outcome.content.as_bytes()]).to_hex()
        [..16]
        .to_string()
}

/// What precedes an outcome the journal answers a repeated call with.
pub const REPLAYED: &str = "(Not done again: an identical call earlier in this session already \
     did this. To do it again on purpose, change its input. It returned:)";

impl Runtime<'_> {
    /// Tells whoever watches the run, if anyone.
    fn seen(&self, seen: &Seen) {
        if let Some(watch) = self.watch {
            watch(seen);
        }
    }

    /// Keeps the session on disk, if the runtime was given a way to.
    fn save(&self, state: &State) {
        if let Some(checkpoint) = self.checkpoint {
            checkpoint(state);
        }
    }

    /// Adds a step to the session's trace.
    fn step(&self, state: &mut State, step: Step) {
        state.steps.push(step);
        if state.steps.len() > MAX_STEPS {
            let over = state.steps.len() - MAX_STEPS;
            state.steps.drain(..over);
        }
    }

    /// A call's step, as it ended.
    fn call_step(
        &self,
        state: &State,
        name: &str,
        key: Option<&str>,
        effect: Option<Effect>,
        outcome: &str,
        start_ms: u64,
    ) -> Step {
        Step {
            kind: "tool".into(),
            name: name.to_string(),
            start_ms,
            ms: wall_ms().saturating_sub(start_ms),
            tokens: None,
            cached: None,
            key: key.map(str::to_string),
            effect,
            outcome: Some(outcome.to_string()),
            tainted: state.standing.tainted((self.now)()),
        }
    }

    /// The tool and input a call names: inside `tools_call`, the inner
    /// call's.
    fn inner<'v>(name: &'v str, input: &'v Value) -> (&'v str, std::borrow::Cow<'v, Value>) {
        if name == TOOLS_CALL {
            let inner = input["name"].as_str().unwrap_or_default();
            let inner_input = input.get("input").cloned().unwrap_or_else(|| json!({}));
            return (inner, std::borrow::Cow::Owned(inner_input));
        }
        (name, std::borrow::Cow::Borrowed(input))
    }

    /// Before a call is checked: if an identical earlier call's outcome is
    /// unknown and its tool can tell what it did from its notes, the
    /// journal is settled (it did it) or the call forgotten (it did not,
    /// and may be made again).
    fn reconcile(&self, state: &mut State, name: &str, input: &Value) {
        let (name, input) = Self::inner(name, input);
        let Some(tool) = self.tools.get(name) else {
            return;
        };
        if tool.dedup() != Dedup::Reconciled {
            return;
        }
        let call_key = key(&state.id, name, &input);
        let Some(entry) = state.journal.entry(&call_key) else {
            return;
        };
        if entry.outcome.certain {
            return;
        }
        let Some(found) = tool.reconcile(&input, &call_key, &entry.notes) else {
            return;
        };
        if !found.certain {
            return;
        }
        if found.is_error {
            state.journal.forget(&call_key);
        } else {
            state.journal.settle(&call_key, found);
        }
        self.save(state);
    }

    /// Answers the calls of a turn that a stopped run (a crash) left
    /// without results, so that the conversation can go on: from the
    /// journal where it recorded them, as unknown where it had written them
    /// down before they ran, and as not recorded otherwise. Whether
    /// anything was repaired.
    fn repair(&self, state: &mut State) -> bool {
        let Some(last) = state.messages.last() else {
            return false;
        };
        if last.role != Role::Assistant {
            return false;
        }
        let calls: Vec<(String, String, Value)> = last
            .tool_uses()
            .map(|(id, name, input)| (id.to_string(), name.to_string(), input.clone()))
            .collect();
        if calls.is_empty() {
            return false;
        }
        let mut results = Vec::new();
        for (use_id, name, input) in &calls {
            let (inner, inner_input) = Self::inner(name, input);
            let call_key = key(&state.id, inner, &inner_input);
            let outcome = match state.journal.entry(&call_key) {
                Some(entry) if entry.outcome.certain => Outcome {
                    content: format!(
                        "(recorded before the run stopped:)\n{}",
                        entry.outcome.content
                    ),
                    ..entry.outcome.clone()
                },
                Some(_) => Outcome::uncertain(crate::journal::IN_FLIGHT),
                None => {
                    let effect = self
                        .tools
                        .get(inner)
                        .map(|tool| tool.effect_of(&inner_input).min(tool.effect()));
                    match effect {
                        Some(Effect::Read) | None => Outcome::error(
                            "not answered: the run stopped during this turn; call it again if \
                             it is still needed",
                        ),
                        Some(_) => Outcome::uncertain(
                            "the run stopped during this turn, and whether this call ran was not \
                             recorded: check before making it again",
                        ),
                    }
                }
            };
            results.push(result_block(use_id, &outcome, &mut state.images));
        }
        state.messages.push(Message {
            role: Role::User,
            content: results,
        });
        state.pending = None;
        true
    }

    /// Runs the session until it ends or must wait.
    pub fn run(&self, state: &mut State) -> Ended {
        self.seen(&Seen::Began(state.id.clone()));
        if self.repair(state) {
            self.save(state);
        }
        let start = RunStart::of(state);
        loop {
            if (self.stopped)() {
                return Ended::Stopped;
            }
            if let Some(pending) = state.pending.clone() {
                let left = self.left(state, &start);
                match self.call(state, &pending.tool, &pending.input, left) {
                    Called::Wait {
                        key,
                        tool,
                        input,
                        reason,
                        ..
                    } => {
                        self.seen(&Seen::Waiting(tool.clone()));
                        return Ended::NeedsApproval {
                            key,
                            tool,
                            input,
                            reason,
                        };
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
            if let Some(steer) = self.steer {
                for said in steer() {
                    state.steer(&said);
                }
            }
            self.compact(state);
            // Within budget: what this call sends and the most it may
            // answer must fit in what is left, besides the reserve for a
            // last answer.
            if let Some(budget) = self.limits.budget_tokens {
                let left = budget.saturating_sub(start.spent(state));
                let need = self.request_tokens(state, self.limits.max_tokens);
                if need.saturating_add(self.limits.reserve_tokens) > left {
                    return self.wrap_up(state, left);
                }
            }
            let start_ms = wall_ms();
            let tainted = state.standing.tainted((self.now)());
            let response = match self.ask(state) {
                Ok(response) => response,
                Err(error) => {
                    return Ended::ModelsFailed {
                        error: error.to_string(),
                    }
                }
            };
            self.step(
                state,
                Step {
                    kind: "model".into(),
                    name: response.model.clone(),
                    start_ms,
                    ms: wall_ms().saturating_sub(start_ms),
                    tokens: Some((response.usage.input, response.usage.output)),
                    cached: response.usage.cached(),
                    key: None,
                    effect: None,
                    outcome: None,
                    tainted,
                },
            );
            state.turns_left -= 1;
            state.turns_taken += 1;
            state.usage.add(&response.usage);
            // Cut off by a stop while it wrote: what it wrote is kept, and
            // the run stops there.
            if response.stop == Stop::Cut {
                let content: Vec<Block> = response
                    .content
                    .into_iter()
                    .filter(|block| matches!(block, Block::Text { text } if !text.is_empty()))
                    .collect();
                if !content.is_empty() {
                    state.messages.push(Message {
                        role: Role::Assistant,
                        content,
                    });
                }
                return Ended::Stopped;
            }
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
                        state.messages.push(Message::user(CONTINUE));
                        continue;
                    }
                    _ => match self.checked(state, &text) {
                        Some(ended) => return ended,
                        None => continue,
                    },
                }
            }
            if let Some(ended) = self.run_calls(state, &calls, &start) {
                return ended;
            }
        }
    }

    /// The calls from `index` on that only read and may each run without
    /// anyone, two or more one after another in the turn (none a batching
    /// tool's, none the same call twice), with where the turn goes on after
    /// them; none if there are fewer than two.
    fn readers(
        &self,
        state: &mut State,
        calls: &[(String, String, Value)],
        index: usize,
    ) -> Option<(Vec<(usize, Ready)>, usize)> {
        // A run told to stop starts nothing more.
        if (self.stopped)() {
            return None;
        }
        let mut group: Vec<(usize, Ready)> = Vec::new();
        let mut next = index;
        while next < calls.len() {
            let (_, name, input) = &calls[next];
            if name == TOOLS_CALL || name == CANNOT_COMPLETE {
                break;
            }
            if next > index {
                self.reconcile(state, name, input);
            }
            match self.check(state, name, input) {
                Checked::Ready(ready)
                    if ready.effect == Effect::Read
                        && self.tools.get(&ready.tool).is_some_and(|t| !t.batches())
                        && group.iter().all(|(_, g)| g.key != ready.key) =>
                {
                    group.push((next, ready));
                    next += 1;
                }
                _ => break,
            }
        }
        (group.len() >= 2).then_some((group, next))
    }

    /// Runs one turn's calls in order, stopping at the first that needs
    /// approval.
    fn run_calls(
        &self,
        state: &mut State,
        calls: &[(String, String, Value)],
        start: &RunStart,
    ) -> Option<Ended> {
        let mut results = Vec::new();
        let mut progress = false;
        let mut made: Vec<Made> = Vec::new();
        let mut index = 0;
        while index < calls.len() {
            let (use_id, name, input) = &calls[index];
            // The model says it cannot complete the task: the run ends
            // there, not done, and the turn's later calls are not made.
            if self.verify.is_some() && name == CANNOT_COMPLETE {
                let why = input["why"].as_str().unwrap_or_default().trim();
                let why = if why.is_empty() {
                    "it gave no reason"
                } else {
                    why
                };
                results.push(result_block(
                    use_id,
                    &Outcome::ok("Noted: the run ends here, as not done."),
                    &mut state.images,
                ));
                for (later, _, _) in &calls[index + 1..] {
                    results.push(result_block(
                        later,
                        &Outcome::error("not run: the task was given up in this turn"),
                        &mut state.images,
                    ));
                }
                state.messages.push(Message {
                    role: Role::User,
                    content: results,
                });
                keep_last_images(state);
                return Some(Ended::CannotComplete {
                    why: why.to_string(),
                });
            }
            self.reconcile(state, name, input);
            // Calls that only read, one after another in the turn, each of
            // which may run without anyone, run side by side: none changes
            // what another reads, and each passed the policy by itself.
            if let Some((group, next)) = self.readers(state, calls, index) {
                let start_ms = wall_ms();
                let held = state.standing.held((self.now)());
                for (_, ready) in &group {
                    self.seen(&Seen::Running(ready.tool.clone()));
                    self.begin(state, ready);
                }
                self.save(state);
                let outcomes: Vec<Outcome> = std::thread::scope(|scope| {
                    let handles: Vec<_> = group
                        .iter()
                        .map(|(_, ready)| {
                            let tool = self.tools.get(&ready.tool).expect("checked");
                            scope.spawn(move || {
                                if held.tainted {
                                    tool.call_tainted(&ready.input, &ready.key)
                                } else {
                                    tool.call(&ready.input, &ready.key)
                                }
                            })
                        })
                        .collect();
                    handles
                        .into_iter()
                        .map(|h| {
                            h.join()
                                .unwrap_or_else(|_| Outcome::error("the tool's call failed"))
                        })
                        .collect()
                });
                for ((at, ready), outcome) in group.iter().zip(outcomes) {
                    let outcome = marked(ready, outcome);
                    let outcome = self.hold(state, &ready.tool, outcome);
                    self.finish(state, ready, &outcome);
                    let step = self.call_step(
                        state,
                        &ready.tool,
                        Some(&ready.key),
                        Some(ready.effect),
                        ended_as(&outcome),
                        start_ms,
                    );
                    self.step(state, step);
                    progress |= !outcome.is_error;
                    made.push(Made {
                        key: ready.key.clone(),
                        tool: ready.tool.clone(),
                        gave: result_digest(&outcome),
                    });
                    results.push(result_block(&calls[*at].0, &outcome, &mut state.images));
                }
                self.save(state);
                index = next;
                continue;
            }
            // The calls of a batching tool that follow this one in the turn,
            // if they too may run, run with it: each passed the policy by
            // itself, and none was decided after seeing another's result.
            if let Checked::Ready(first) = self.check(state, name, input) {
                let tool = self.tools.get(&first.tool).expect("checked");
                if tool.batches() && name != TOOLS_CALL {
                    let mut group = vec![(index, first)];
                    let mut next = index + 1;
                    while next < calls.len() && calls[next].1 == *name {
                        self.reconcile(state, name, &calls[next].2);
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
                    self.seen(&Seen::Running(group[0].1.tool.clone()));
                    let start_ms = wall_ms();
                    let held = state.standing.held((self.now)());
                    for (_, ready) in &group {
                        self.begin(state, ready);
                    }
                    self.save(state);
                    // Subagents work within what is left of the budget.
                    let left = self.left(state, start);
                    let mut outcomes = tool.call_batch_budgeted(&inputs, held, left);
                    outcomes.resize_with(group.len(), || {
                        Outcome::uncertain("the tool gave no outcome for this call")
                    });
                    for ((at, ready), outcome) in group.iter().zip(outcomes) {
                        let outcome = marked(ready, outcome);
                        let outcome = self.hold(state, &ready.tool, outcome);
                        self.finish(state, ready, &outcome);
                        let step = self.call_step(
                            state,
                            &ready.tool,
                            Some(&ready.key),
                            Some(ready.effect),
                            ended_as(&outcome),
                            start_ms,
                        );
                        self.step(state, step);
                        progress |= !outcome.is_error;
                        made.push(Made {
                            key: ready.key.clone(),
                            tool: ready.tool.clone(),
                            gave: result_digest(&outcome),
                        });
                        results.push(result_block(&calls[*at].0, &outcome, &mut state.images));
                    }
                    self.save(state);
                    index = next;
                    continue;
                }
            }
            index += 1;
            let left = self.left(state, start);
            match self.call(state, name, input, left) {
                Called::Ran(outcome) => {
                    progress |= !outcome.is_error;
                    let (inner, inner_input) = Self::inner(name, input);
                    made.push(Made {
                        key: key(&state.id, inner, &inner_input),
                        tool: inner.to_string(),
                        gave: result_digest(&outcome),
                    });
                    results.push(result_block(use_id, &outcome, &mut state.images));
                }
                Called::Wait {
                    key,
                    tool,
                    input: inner,
                    reason,
                    effect,
                } => {
                    self.seen(&Seen::Waiting(tool.clone()));
                    results.push(result_block(
                        use_id,
                        &Outcome::ok(WAITING),
                        &mut state.images,
                    ));
                    for (later, _, _) in &calls[index..] {
                        results.push(result_block(
                            later,
                            &Outcome::error("not run: an earlier call in this turn waited for approval; call it again if it is still needed"),
                            &mut state.images,
                        ));
                    }
                    state.messages.push(Message {
                        role: Role::User,
                        content: results,
                    });
                    keep_last_images(state);
                    state.pending = Some(Pending {
                        use_id: use_id.clone(),
                        key: key.clone(),
                        tool: name.clone(),
                        input: input.clone(),
                        effect: Some(effect),
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
        // Going round in circles: the model is told, and if it goes on,
        // the run stops.
        let mut stuck = None;
        state.recent.extend(made);
        if state.recent.len() > RECENT_KEPT {
            let over = state.recent.len() - RECENT_KEPT;
            state.recent.drain(..over);
        }
        if let Some(circling) = circling(&state.recent, self.limits.stuck_after) {
            let count = circling.count;
            let (what, did) = match circling.tools.as_slice() {
                [a, b] => (
                    format!("{a} and {b}"),
                    format!("took turns with the same results {count} times"),
                ),
                tools => (
                    tools.join(", "),
                    format!("gave the same result {count} times in a row"),
                ),
            };
            if circling.stop {
                stuck = Some(Ended::Stuck {
                    why: format!("{what} {did}"),
                });
            } else {
                progress = false;
                note_to_model(
                    &mut results,
                    &format!(
                        "[{what} {did}: calling again will not change that. Change your \
                         approach, or say what stops you.]"
                    ),
                );
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
        keep_last_images(state);
        stuck
    }

    /// What is left of the run's token budget, the reserve for a last
    /// answer kept back, if the run has a budget.
    fn left(&self, state: &State, start: &RunStart) -> Option<u64> {
        self.limits.budget_tokens.map(|budget| {
            budget
                .saturating_sub(start.spent(state))
                .saturating_sub(self.limits.reserve_tokens)
        })
    }

    /// The model said it is done: without a verifier, the run is done.
    /// With one, its answer is checked: passed, the run is done; failed,
    /// the model is told what the check reported and goes on (`None`),
    /// unless that was the last check allowed, or the turns are out, and
    /// the run ends unverified with the best answer.
    fn checked(&self, state: &mut State, text: &str) -> Option<Ended> {
        let Some(check) = self.verify else {
            return Some(Ended::Done {
                text: text.to_string(),
            });
        };
        self.seen(&Seen::Checking);
        let start_ms = wall_ms();
        let verdict = check(state, text);
        let step = self.call_step(
            state,
            verify::CHECK,
            None,
            None,
            if verdict.passed { "ok" } else { "error" },
            start_ms,
        );
        self.step(state, step);
        state.attempts.record(text, verdict.clone());
        if verdict.passed {
            return Some(Ended::Done {
                text: text.to_string(),
            });
        }
        let checked = state.attempts.checked;
        if checked >= self.limits.verify_attempts || state.turns_left == 0 {
            let best = state.attempts.best.clone()?;
            return Some(Ended::Unverified {
                text: best.text,
                attempt: best.number,
                why: verify::last_line(&verdict.report),
            });
        }
        state.messages.push(Message::user(verify::failed(
            &verdict,
            checked,
            self.limits.verify_attempts,
        )));
        None
    }

    /// The tools the model is shown: the toolbox's, and in quarantine the
    /// reader.
    fn shown(&self) -> Vec<ToolSpec> {
        let mut shown = self.tools.shown();
        if self.policy.quarantine {
            shown.push(quarantine::spec());
        }
        if self.verify.is_some() {
            shown.push(verify::spec());
        }
        shown
    }

    /// In quarantine, what a tool returned from outside is held out of the
    /// conversation, and the model is told how to read it; the session is
    /// not tainted by it. A tool whose results the model acts on, an error,
    /// and pictures are not held.
    fn hold(&self, state: &mut State, tool: &str, mut outcome: Outcome) -> Outcome {
        let holds = self.tools.get(tool).is_some_and(|t| t.quarantinable());
        if !self.policy.quarantine
            || !holds
            || !outcome.untrusted
            || outcome.is_error
            || !outcome.pictures.is_empty()
        {
            return outcome;
        }
        let held = Held {
            handle: format!("q{}", state.held.len() + 1),
            tool: tool.to_string(),
            content: std::mem::take(&mut outcome.content),
            at: (self.now)(),
        };
        outcome.content = quarantine::placeholder(&held);
        outcome.untrusted = false;
        state.held.push(held);
        outcome
    }

    /// Answers [`QUARANTINE_READ`]: a reader without tools reads the content
    /// held, and its answer, held to the schema, is what the model sees. An
    /// answer with free text taints the session; the strings it gives are
    /// labelled as from outside ([`State::outside_values`]).
    fn read_held(&self, state: &mut State, input: &Value, left: Option<u64>) -> Outcome {
        let handle = input["handle"].as_str().unwrap_or_default().trim();
        let question = input["question"].as_str().unwrap_or_default().trim();
        let schema = &input["schema"];
        let Some(held) = state.held.iter().find(|h| h.handle == handle).cloned() else {
            return Outcome::error(format!("nothing is held as {handle:?}"));
        };
        if question.is_empty() {
            return Outcome::error("ask the reader a question");
        }
        let carries = match quarantine::check_schema(schema) {
            Ok(carries) => carries,
            Err(why) => return Outcome::error(format!("not a schema the reader answers: {why}")),
        };
        let request = quarantine::request(&held, question, schema);
        let need = (request.system.len()
            + request.messages.iter().map(Message::chars).sum::<usize>()) as u64
            / 3
            + u64::from(request.max_tokens);
        if let Some(left) = left.filter(|left| need > *left) {
            return Outcome::error(format!(
                "not read: it takes about {need} tokens, and {left} are left of the run's budget"
            ));
        }
        let start_ms = wall_ms();
        let tainted = state.standing.tainted((self.now)());
        let answered = self.ask_reader(state, &request);
        let response = match answered {
            Ok(response) => response,
            Err(error) => return Outcome::error(format!("the reader could not answer: {error}")),
        };
        state.usage.add(&response.usage);
        self.step(
            state,
            Step {
                kind: "model".into(),
                name: response.model.clone(),
                start_ms,
                ms: wall_ms().saturating_sub(start_ms),
                tokens: Some((response.usage.input, response.usage.output)),
                cached: response.usage.cached(),
                key: None,
                effect: None,
                outcome: Some("reader".into()),
                tainted,
            },
        );
        let text = Message {
            role: Role::Assistant,
            content: response.content,
        }
        .text();
        let answer = match quarantine::answer_of(&text)
            .and_then(|answer| quarantine::validate(&answer, schema).map(|()| answer))
        {
            Ok(answer) => answer,
            Err(why) => {
                return Outcome::error(format!(
                    "the reader's answer does not keep to the schema, so it is not shown: {why}"
                ))
            }
        };
        state
            .outside_values
            .extend(quarantine::labels(&answer, schema));
        let free = carries == quarantine::Carries::Text && quarantine::has_text(&answer, schema);
        if free {
            state.standing.taint(&self.policy, (self.now)());
            Outcome::ok(format!(
                "[read from {handle} without tools; its free text is content from outside]\n{answer}"
            ))
        } else {
            Outcome::ok(format!("[read from {handle} without tools]\n{answer}"))
        }
    }

    /// The reader's model call: the session's models in turn, as a run asks
    /// them, without tools and unwatched (what it writes is not the
    /// agent's reply).
    fn ask_reader(&self, state: &mut State, request: &Request) -> Result<Response, ModelError> {
        let now = (self.now)();
        let mut last = ModelError::Refused("no model is configured".into());
        for (index, model) in self.models.iter().enumerate() {
            if state.cooling.get(&index).is_some_and(|until| now < *until) {
                continue;
            }
            match model.complete(request) {
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
        Err(last)
    }

    /// Tokens the next model call may cost at most: what it is sent, as an
    /// estimate (a token for every three bytes, which counts English text
    /// high), and the most it may answer.
    fn request_tokens(&self, state: &State, max_tokens: u32) -> u64 {
        let tools: usize = self
            .shown()
            .iter()
            .map(|t| t.name.len() + t.description.len() + t.input_schema.to_string().len())
            .sum();
        let chars =
            self.system.len() + state.messages.iter().map(Message::chars).sum::<usize>() + tools;
        (chars as u64).div_ceil(3) + u64::from(max_tokens)
    }

    /// The last answer of a run whose budget is spent: within what is
    /// `left`, the model is asked to say what it did and what remains, and
    /// its calls, if it makes any, are not made.
    fn wrap_up(&self, state: &mut State, left: u64) -> Ended {
        match state.messages.last_mut() {
            Some(last) if last.role == Role::User => note_to_model(&mut last.content, BUDGET_SPENT),
            _ => state.messages.push(Message::user(BUDGET_SPENT)),
        }
        let room = left.saturating_sub(self.request_tokens(state, 0));
        if room < 256 {
            return Ended::OutOfBudget {
                text: String::new(),
            };
        }
        let max = room.min(u64::from(self.limits.max_tokens)) as u32;
        let start_ms = wall_ms();
        match self.ask_with(state, max) {
            Ok(response) => {
                self.step(
                    state,
                    Step {
                        kind: "model".into(),
                        name: response.model.clone(),
                        start_ms,
                        ms: wall_ms().saturating_sub(start_ms),
                        tokens: Some((response.usage.input, response.usage.output)),
                        cached: response.usage.cached(),
                        key: None,
                        effect: None,
                        outcome: None,
                        tainted: state.standing.tainted((self.now)()),
                    },
                );
                state.turns_taken += 1;
                state.usage.add(&response.usage);
                // Its words only: a call it asks for is not made.
                let content: Vec<Block> = response
                    .content
                    .into_iter()
                    .filter(|block| matches!(block, Block::Text { text } if !text.is_empty()))
                    .collect();
                let text = Message {
                    role: Role::Assistant,
                    content: content.clone(),
                }
                .text();
                if !content.is_empty() {
                    state.messages.push(Message {
                        role: Role::Assistant,
                        content,
                    });
                }
                Ended::OutOfBudget { text }
            }
            Err(_) => Ended::OutOfBudget {
                text: String::new(),
            },
        }
    }

    /// Approves the call waiting in `state`: the next run makes it.
    pub fn approve(&self, state: &mut State) -> bool {
        self.approve_as(state, "", Scope::Once).is_ok()
    }

    /// Approves the call waiting in `state`, as `by` (who the channel says
    /// decided), and, as `scope` says, the calls like it for the rest of the
    /// session. A payment is approved one at a time, a tainted session
    /// allows nothing for the rest of it, and a rule must be for the waiting
    /// call's tool and match it. The decision is kept with the session
    /// ([`State::decisions`]).
    pub fn approve_as(&self, state: &mut State, by: &str, scope: Scope) -> Result<(), String> {
        let Some(pending) = state.pending.clone() else {
            return Err("nothing waits for a decision".into());
        };
        let now = (self.now)();
        let (inner, inner_input) = Self::inner(&pending.tool, &pending.input);
        let allowance = match scope {
            Scope::Once => None,
            _ if pending.effect.unwrap_or(Effect::Spend) >= Effect::Spend => {
                return Err("a payment is approved one at a time: approve it once".into())
            }
            // What is allowed would not hold while the taint lasts.
            _ if state.standing.tainted(now) => {
                return Err(
                    "the session has read content from outside, so nothing is allowed \
                            for the rest of it: approve this call once"
                        .into(),
                )
            }
            // Nor once the session holds confidential content.
            _ if state.standing.confidential => {
                return Err(
                    "the session holds confidential content, so nothing is allowed for the \
                     rest of it: approve this call once"
                        .into(),
                )
            }
            // Nor would an allowance for an argument read from content held
            // in quarantine: a person approves each call that uses one.
            _ if quarantine::carried(&pending.input, &state.outside_values).is_some() => {
                return Err(
                    "an argument comes from content held in quarantine, so each call \
                     that uses it is approved on its own: approve this call once"
                        .into(),
                )
            }
            Scope::Call => Some(crate::policy::Allowance {
                tool: inner.to_string(),
                input: Some(inner_input.into_owned()),
                rule: None,
                by: by.to_string(),
                at: now,
            }),
            Scope::Rule(rule) => {
                if rule.tool != inner || !rule.matches(inner, &inner_input) {
                    return Err(format!(
                        "{} does not match the call waiting ({inner})",
                        rule.describe()
                    ));
                }
                Some(crate::policy::Allowance {
                    tool: inner.to_string(),
                    input: None,
                    rule: Some(rule),
                    by: by.to_string(),
                    at: now,
                })
            }
        };
        state.standing.approve(&self.policy, &pending.key, now);
        state.decisions.push(Decided {
            request: pending.request().to_string(),
            tool: inner.to_string(),
            approved: true,
            by: by.to_string(),
            at: now,
            allowed: allowance.as_ref().map(|a| a.describe()),
            why: None,
            signature: None,
        });
        if let Some(allowance) = allowance {
            state.standing.allowed.push(allowance);
        }
        Ok(())
    }

    /// Declines the call waiting in `state`: the model is told why, and the
    /// next run continues without it.
    pub fn decline(&self, state: &mut State, why: &str) -> bool {
        self.decline_as(state, "", why)
    }

    /// [`Runtime::decline`], as `by`; the decision is kept with the session.
    pub fn decline_as(&self, state: &mut State, by: &str, why: &str) -> bool {
        match state.pending.take() {
            Some(pending) => {
                replace_result(
                    state,
                    &pending.use_id,
                    &Outcome::error(format!("a person declined this call: {why}")),
                );
                let (inner, _) = Self::inner(&pending.tool, &pending.input);
                state.decisions.push(Decided {
                    request: pending.request().to_string(),
                    tool: inner.to_string(),
                    approved: false,
                    by: by.to_string(),
                    at: (self.now)(),
                    allowed: None,
                    why: Some(why.to_string()),
                    signature: None,
                });
                true
            }
            None => false,
        }
    }

    fn ask(&self, state: &mut State) -> Result<Response, ModelError> {
        self.ask_with(state, self.limits.max_tokens)
    }

    /// [`Runtime::ask`], the answer at most `max_tokens` long.
    fn ask_with(&self, state: &mut State, max_tokens: u32) -> Result<Response, ModelError> {
        let request = Request {
            system: self.system.clone(),
            messages: state.messages.clone(),
            tools: self.shown(),
            max_tokens,
            images: state.images.clone(),
        };
        let now = (self.now)();
        let mut last = ModelError::Refused("no model is configured".into());
        let mut asked = false;
        for (index, model) in self.models.iter().enumerate() {
            if state.cooling.get(&index).is_some_and(|until| now < *until) {
                continue;
            }
            asked = true;
            let answered = match self.watch {
                Some(watch) => {
                    watch(&Seen::Asking);
                    // Stopped while it writes, the model is cut off there.
                    let answered = model.stream(&request, &mut |delta| {
                        watch(&match delta {
                            Delta::Text(text) => Seen::Text(text),
                            Delta::Call(tool) => Seen::Calling(tool),
                        });
                        !(self.stopped)()
                    });
                    watch(&match &answered {
                        Ok(response) => Seen::Answered(response.stop.clone()),
                        Err(_) => Seen::Dropped,
                    });
                    answered
                }
                None => model.complete(&request),
            };
            match answered {
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

    fn call(&self, state: &mut State, name: &str, input: &Value, left: Option<u64>) -> Called {
        if name == QUARANTINE_READ && self.policy.quarantine {
            return Called::Ran(self.read_held(state, input, left));
        }
        self.reconcile(state, name, input);
        let start_ms = wall_ms();
        match self.check(state, name, input) {
            Checked::Answered(outcome) => {
                let (inner, _) = Self::inner(name, input);
                let step = self.call_step(state, inner, None, None, "answered", start_ms);
                self.step(state, step);
                Called::Ran(outcome)
            }
            Checked::Wait {
                key,
                tool,
                input,
                reason,
                effect,
            } => {
                let step =
                    self.call_step(state, &tool, Some(&key), Some(effect), "waiting", start_ms);
                self.step(state, step);
                Called::Wait {
                    key,
                    tool,
                    input,
                    reason,
                    effect,
                }
            }
            Checked::Ready(ready) => {
                let tool = self.tools.get(&ready.tool).expect("checked");
                self.seen(&Seen::Running(ready.tool.clone()));
                let held = state.standing.held((self.now)());
                let written = self.begin(state, &ready);
                if written {
                    self.save(state);
                }
                let outcome = if tool.batches() {
                    // A batching tool's call is a batch of one, within what
                    // is left of the budget (subagents at work).
                    tool.call_batch_budgeted(&[(&ready.input, ready.key.as_str())], held, left)
                        .pop()
                        .unwrap_or_else(|| {
                            Outcome::uncertain("the tool gave no outcome for this call")
                        })
                } else {
                    let mut note = |text: &str| {
                        state.journal.note(&ready.key, text);
                        self.save(state);
                    };
                    tool.call_noting(&ready.input, &ready.key, held.tainted, &mut note)
                };
                let outcome = marked(&ready, outcome);
                let outcome = self.hold(state, &ready.tool, outcome);
                self.finish(state, &ready, &outcome);
                let step = self.call_step(
                    state,
                    &ready.tool,
                    Some(&ready.key),
                    Some(ready.effect),
                    ended_as(&outcome),
                    start_ms,
                );
                self.step(state, step);
                if written {
                    self.save(state);
                }
                Called::Ran(outcome)
            }
        }
    }

    /// Writes a journaled call that changes something down before it runs
    /// ([`Journal::begin`]); whether it was.
    fn begin(&self, state: &mut State, ready: &Ready) -> bool {
        if !ready.journaled || ready.effect == Effect::Read {
            return false;
        }
        state.journal.begin(
            &ready.key,
            Entry {
                tool: ready.tool.clone(),
                input: ready.input.clone(),
                effect: ready.effect,
                outcome: Outcome::ok(""),
                at: (self.now)(),
                notes: Vec::new(),
                dedup: Some(ready.dedup),
            },
        );
        true
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
        if name == QUARANTINE_READ {
            return Checked::Answered(Outcome::error(
                "nothing is held in quarantine: this session does not hold content from outside",
            ));
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
        let dedup = tool.dedup();
        let mut retried = false;
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
                match dedup {
                    // Made again: the tool keeps it from happening twice.
                    Dedup::Idempotent | Dedup::Keyed => retried = true,
                    Dedup::Reconciled => return Checked::Answered(Outcome::uncertain(
                        "not repeated yet: an identical earlier call may or may not have taken \
                         effect, and what it did cannot be told yet; ask again later",
                    )),
                    Dedup::AtLeastOnce => {
                        return Checked::Answered(Outcome::uncertain(
                            "not repeated: an identical earlier call may or may not have taken \
                         effect; check its result before trying again",
                        ))
                    }
                }
            }
            _ => {}
        }
        let now = (self.now)();
        // A call can do at most what its tool declares.
        let effect = tool.effect_of(input).min(tool.effect());
        let mut pre_approved = None;
        let decision = self
            .policy
            .decide(&state.standing, name, effect, &call_key, now);
        // An external call or a payment whose input carries what was read
        // from content held in quarantine waits for a person, whatever the
        // level, a skill or an allowance says, unless they approved it.
        let approved = state
            .standing
            .approvals
            .get(&call_key)
            .is_some_and(|until| now < *until);
        if effect >= Effect::External && !approved && !matches!(decision, Decision::Deny(_)) {
            if let Some(value) = quarantine::carried(input, &state.outside_values) {
                let shown: String = value.chars().take(40).collect();
                return Checked::Wait {
                    key: call_key,
                    tool: name.to_string(),
                    input: input.clone(),
                    reason: format!(
                        "an argument comes from content held in quarantine ({shown}{})",
                        if value.chars().count() > 40 {
                            "…"
                        } else {
                            ""
                        }
                    ),
                    effect,
                };
            }
        }
        match decision {
            Decision::Deny(reason) => {
                return Checked::Answered(Outcome::error(format!("refused by policy: {reason}")))
            }
            Decision::Ask(reason) => {
                // A skill the session read pre-approves the call, or a
                // person allowed it for the session when they approved
                // another, and the session has read nothing from outside
                // since: no one is asked. What the level refuses stays
                // refused, and spending is never pre-approved: a person
                // approves each payment, or the operator's autonomy level
                // does.
                let open = effect < Effect::Spend && state.standing.open(now);
                let by_skill = state
                    .standing
                    .pre_approving(name, input)
                    .filter(|_| open && self.policy.skill_approvals)
                    .map(|(skill, rule)| {
                        format!("pre-approved by skill {skill}: {}", rule.describe())
                    });
                let by_person = state
                    .standing
                    .allowing(name, input)
                    .filter(|_| open)
                    .map(|a| format!("allowed for this session by {}: {}", a.by, a.describe()));
                match by_skill.or(by_person) {
                    Some(by) => pre_approved = Some(by),
                    None => {
                        return Checked::Wait {
                            key: call_key,
                            tool: name.to_string(),
                            input: input.clone(),
                            reason,
                            effect,
                        }
                    }
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
            dedup,
            retried,
            pre_approved,
        })
    }

    /// Records what a call that ran did: its taint, its failure, and, if it
    /// is journaled, its outcome.
    fn finish(&self, state: &mut State, ready: &Ready, outcome: &Outcome) {
        let now = (self.now)();
        if let Some(spent) = outcome.spent {
            state.delegated.add(&spent);
        }
        if outcome.untrusted {
            state.standing.taint(&self.policy, now);
        }
        if outcome.confidential {
            state.standing.hold_confidential();
        }
        // What is now so of what the call read or changed, by its name.
        if let Some(name) = self
            .tools
            .get(&ready.tool)
            .and_then(|tool| tool.resource(&ready.input))
            .filter(|_| !outcome.is_error)
        {
            let at = state.latest.values().map(|l| l.at).max().unwrap_or(0) + 1;
            state.latest.insert(
                name,
                Latest {
                    tool: ready.tool.clone(),
                    outcome: shorten(&outcome.content, 200),
                    at,
                },
            );
            if state.latest.len() > LATEST_KEPT {
                let oldest = state
                    .latest
                    .iter()
                    .min_by_key(|(_, l)| l.at)
                    .map(|(name, _)| name.clone());
                if let Some(oldest) = oldest {
                    state.latest.remove(&oldest);
                }
            }
        }
        // What lasts is kept apart from the conversation, so that no
        // summary of it loses it.
        if ready.effect >= Effect::Write && !outcome.is_error {
            state.done.push(Done {
                tool: ready.tool.clone(),
                input: shorten(&ready.input.to_string(), 160),
                unknown: !outcome.certain,
            });
            if state.done.len() > LASTING_KEPT {
                let gone = state.done.remove(0);
                *state.done_before.entry(gone.tool).or_default() += 1;
            }
        }
        if let (Some((skill, rules)), false) = (&outcome.pre_approval, outcome.is_error) {
            state
                .standing
                .pre_approved
                .insert(skill.clone(), rules.clone());
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
                    notes: Vec::new(),
                    dedup: Some(ready.dedup),
                },
            );
        }
    }

    /// Keeps a long conversation within the limits: past
    /// [`Limits::compact_chars`], the results of calls long past are cut
    /// first ([`mask_old_results`]); still past it, or past
    /// [`Limits::compact_messages`], the middle is replaced by a summary,
    /// keeping the task and the latest messages.
    fn compact(&self, state: &mut State) {
        let chars = |state: &State| -> usize { state.messages.iter().map(Message::chars).sum() };
        if chars(state) > self.limits.compact_chars
            && mask_old_results(state, self.limits.keep_last) > 0
        {
            state.compactions += 1;
        }
        if state.messages.len() <= self.limits.compact_messages
            && chars(state) <= self.limits.compact_chars
        {
            return;
        }
        compact_now(state, self.limits.keep_last);
    }
}

/// The longest result of a call long past that is kept whole when a
/// conversation is cut down ([`mask_old_results`]), in bytes.
pub const MASK_OVER: usize = 400;

/// What a result cut from a conversation begins with.
pub const MASKED: &str = "[Cut to save context:";

/// Cuts the results of the calls before the last `keep_last` messages that
/// are longer than [`MASK_OVER`]: the calls stay, and each result becomes a
/// line saying what it was and how long. The call can be made again: one
/// the journal recorded is answered from it, as it was. How many results
/// were cut.
pub fn mask_old_results(state: &mut State, keep_last: usize) -> usize {
    let until = state.messages.len().saturating_sub(keep_last);
    // Which tool each call was, to name what a result was of.
    let mut tools: BTreeMap<String, String> = BTreeMap::new();
    let mut cut = 0;
    for message in &mut state.messages[..until] {
        for block in &mut message.content {
            match block {
                Block::ToolUse { id, name, .. } => {
                    tools.insert(id.clone(), name.clone());
                }
                Block::ToolResult { id, content, .. }
                    if content.len() > MASK_OVER && !content.starts_with(MASKED) =>
                {
                    let tool = tools.get(id.as_str()).map_or("a call", String::as_str);
                    *content = format!(
                        "{MASKED} the result of {tool}, {} characters. Make the call again if \
                         you need it.]",
                        content.chars().count()
                    );
                    cut += 1;
                }
                _ => {}
            }
        }
    }
    cut
}

/// The most calls a session keeps of what it did that lasts
/// ([`State::done`]): the last ones, the others counted by tool.
pub const LASTING_KEPT: usize = 40;

/// A call the session made that lasts ([`State::done`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Done {
    /// Its tool.
    pub tool: String,
    /// Its input, shortened.
    pub input: String,
    /// Whether its outcome is unknown: it may or may not have taken effect.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unknown: bool,
}

/// The latest outcome of a thing a session read or changed
/// ([`State::latest`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Latest {
    /// The tool whose call it was.
    pub tool: String,
    /// What the call returned, shortened.
    pub outcome: String,
    /// Its order among the session's latest outcomes (the higher, the
    /// later).
    pub at: u64,
}

/// How many things [`State::latest`] keeps the latest outcome of.
pub const LATEST_KEPT: usize = 32;

/// What is now so of what the session read or changed, as a summary
/// carries it after what lasts: the latest outcome of each, the most recent
/// last. Empty if there is none.
pub fn latest(state: &State) -> String {
    if state.latest.is_empty() {
        return String::new();
    }
    let mut by_time: Vec<(&String, &Latest)> = state.latest.iter().collect();
    by_time.sort_by_key(|(_, l)| l.at);
    let mut lines = vec![
        "[The latest of what this session read or changed, each as its last call left it:]"
            .to_string(),
    ];
    for (name, latest) in by_time {
        lines.push(format!(
            "- {name} ({}): {}",
            latest.tool,
            latest.outcome.replace('\n', " ")
        ));
    }
    lines.join("\n")
}

/// What the session has done that lasts, as a summary begins: its calls
/// that changed something or reached outside and did not fail, oldest
/// first, those let go counted by tool. Empty if there are none.
pub fn lasting(state: &State) -> String {
    if state.done.is_empty() && state.done_before.is_empty() {
        return String::new();
    }
    let mut lines = vec!["[What this session has done that lasts, oldest first:]".to_string()];
    if !state.done_before.is_empty() {
        let counted: Vec<String> = state
            .done_before
            .iter()
            .map(|(tool, n)| format!("{n} {tool}"))
            .collect();
        lines.push(format!("- before these, calls of: {}", counted.join(", ")));
    }
    for done in &state.done {
        lines.push(format!(
            "- {} {}{}",
            done.tool,
            done.input,
            if done.unknown {
                " (whether it took effect is unknown)"
            } else {
                ""
            }
        ));
    }
    lines.join("\n")
}

/// Compacts `state`'s conversation now, whatever its size: everything but
/// the task and the last `keep_last` messages becomes a summary. Whether
/// anything was compacted.
pub fn compact_now(state: &mut State, keep_last: usize) -> bool {
    let len = state.messages.len();
    let keep = keep_last.min(len.saturating_sub(2));
    // Start the kept tail on a message that is not a tool result, so that
    // no result is kept without the call it answers.
    let mut tail = len - keep;
    while tail > 1 && tail < len && is_results(&state.messages[tail]) {
        tail -= 1;
    }
    // Nothing to compact: at most the task before the tail, or no tail.
    if tail <= 1 || tail >= len {
        return false;
    }
    // What lasts first, kept apart from the conversation, so that no
    // summary of a summary loses it; then what the compacted messages said.
    let kept: Vec<String> = [lasting(state), latest(state)]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect();
    let summary = match kept.join("\n") {
        kept if kept.is_empty() => summarize(&state.messages[1..tail]),
        kept => format!("{kept}\n{}", summarize(&state.messages[1..tail])),
    };
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
    keep_last_images(state);
    true
}

fn is_results(message: &Message) -> bool {
    message
        .content
        .iter()
        .any(|b| matches!(b, Block::ToolResult { .. }))
}

/// How a call ended, for its step: `ok`, `error` or `unknown`.
fn ended_as(outcome: &Outcome) -> &'static str {
    match (outcome.certain, outcome.is_error) {
        (false, _) => "unknown",
        (true, true) => "error",
        (true, false) => "ok",
    }
}

/// The block that gives the model `outcome`: its pictures by their
/// digests, their bytes kept in `images`.
fn result_block(use_id: &str, outcome: &Outcome, images: &mut BTreeMap<String, String>) -> Block {
    let refs = outcome
        .pictures
        .iter()
        .map(|picture| {
            let digest = image_digest(&picture.media_type, &picture.base64);
            images.insert(digest.clone(), picture.base64.clone());
            Image {
                media_type: picture.media_type.clone(),
                digest,
            }
        })
        .collect();
    Block::ToolResult {
        id: use_id.to_string(),
        content: outcome.content.clone(),
        is_error: outcome.is_error,
        images: refs,
    }
}

/// Lets go of the bytes of all but the last [`MAX_IMAGES_KEPT`] pictures
/// the conversation names and the person's last
/// ([`crate::model::person_pictures`]), and of any it no longer names.
fn keep_last_images(state: &mut State) {
    let images = images_of(&state.messages);
    let mut kept: Vec<String> = Vec::new();
    for image in images.iter().rev() {
        if kept.len() == MAX_IMAGES_KEPT {
            break;
        }
        if !kept.contains(&image.digest) {
            kept.push(image.digest.clone());
        }
    }
    for at in crate::model::person_pictures(&state.messages) {
        if let Some(image) = images.get(at) {
            kept.push(image.digest.clone());
        }
    }
    state.images.retain(|digest, _| kept.contains(digest));
}

fn replace_result(state: &mut State, use_id: &str, outcome: &Outcome) {
    let mut found = None;
    for (m, message) in state.messages.iter().enumerate().rev() {
        if let Some(b) = message
            .content
            .iter()
            .position(|block| matches!(block, Block::ToolResult { id, .. } if id == use_id))
        {
            found = Some((m, b));
            break;
        }
    }
    if let Some((m, b)) = found {
        let block = result_block(use_id, outcome, &mut state.images);
        state.messages[m].content[b] = block;
        keep_last_images(state);
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
                Block::Image { image } => format!("- I was shown a picture ({})", image.media_type),
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
