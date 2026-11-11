//! Model providers over HTTPS, through `sion-http` (bounded, guarded, with
//! the hybrid post-quantum key exchange first): the Anthropic Messages API,
//! any OpenAI-compatible chat completions API, and Google's Gemini API.
//!
//! All stream ([`Model::stream`]): the request asks for Server-Sent Events,
//! and the text is told as each event brings it, while the answer is built
//! from the same events into what a whole answer would have been. An
//! answer that stops before its end is a transport failure, and an error
//! the stream reports partway is mapped as the status it stands for. A
//! server that answers whole although asked to stream is read whole.
//!
//! Each takes one API key or several ([`Keys`]): a key the provider limits
//! or refuses rests, and the next is tried.
//!
//! What a model thought is kept ([`Block::Reasoning`]) and given back to
//! it, so that a model calling tool after tool goes on from its own
//! reasoning: Anthropic's thinking blocks, signed, OpenAI's reasoning items,
//! encrypted (`include: ["reasoning.encrypted_content"]`, since nothing is
//! stored with OpenAI), the signature Gemini puts on its text as on its
//! calls, and the reasoning text a chat completions server returns, given
//! back within the task it was for. Each provider gives back only its own,
//! by its kind, base URL and model ([`Anthropic::source`]). A request that
//! carries kept reasoning and is refused as malformed (HTTP 400) is sent
//! once more without it: reasoning that no longer checks is let go rather
//! than failing the call. How hard a model thinks ([`Thinking`]) is each
//! provider's `thinking`, with room to think besides the answer
//! ([`most_tokens`]).

use serde_json::{json, Value};

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::mcp_http::Events;
use crate::model::{
    cut_off, images_shown, keeps_reasoning, keeps_signatures, most_tokens, reasoning_from,
    tell_whole, without_reasoning, Block, Delta, Effort, Image, Message, Model, ModelError,
    Request, Response, Role, Stop, Thinking, Usage,
};

/// Sends `request` with `send`, and, if the provider refuses it as
/// malformed (HTTP 400) while it carries reasoning kept from `from`, once
/// more without that reasoning: reasoning that no longer checks (another
/// key's organization, a changed model) is let go rather than failing the
/// call. What comes back is marked as `from`'s.
pub(crate) fn kept_or_not(
    request: &Request,
    from: &str,
    send: impl FnMut(&Request) -> Result<Response, ModelError>,
) -> Result<Response, ModelError> {
    let kept = keeps_reasoning(&request.messages, from);
    kept_or_without(request, from, kept, send)
}

/// [`kept_or_not`], for a provider that is also sent the signatures on the
/// conversation's calls (Gemini): a request that carries any is sent once
/// more without them, whoever they came from.
pub(crate) fn signed_or_not(
    request: &Request,
    from: &str,
    send: impl FnMut(&Request) -> Result<Response, ModelError>,
) -> Result<Response, ModelError> {
    let kept = keeps_reasoning(&request.messages, from) || keeps_signatures(&request.messages);
    kept_or_without(request, from, kept, send)
}

fn kept_or_without(
    request: &Request,
    from: &str,
    kept: bool,
    mut send: impl FnMut(&Request) -> Result<Response, ModelError>,
) -> Result<Response, ModelError> {
    let answered = match send(request) {
        Err(ModelError::Refused(why)) if why.starts_with("HTTP 400") && kept => {
            // Answered without it, the reasoning kept is of no use to send
            // again: the session lets go of it.
            send(&Request {
                messages: without_reasoning(&request.messages),
                ..request.clone()
            })
            .map(|response| Response {
                forgot_reasoning: true,
                ..response
            })
        }
        answered => answered,
    };
    answered.map(|response| reasoning_from(response, from))
}

/// The reasoning `block` keeps from `from`, if it is reasoning, and kept
/// from there.
fn kept_from<'a>(block: &'a Block, from: &str) -> Option<&'a serde_json::Value> {
    match block {
        Block::Reasoning { from: kept, data } if kept == from => Some(data),
        _ => None,
    }
}

/// Where in `messages` the task at hand began: after the last message from
/// the person that is not tool results. Reasoning a chat completions server
/// returned is given back from there on, as those servers ask.
fn task_start(messages: &[Message]) -> usize {
    messages
        .iter()
        .rposition(|m| {
            m.role == Role::User
                && !m
                    .content
                    .iter()
                    .any(|b| matches!(b, Block::ToolResult { .. }))
        })
        .map_or(0, |at| at + 1)
}

/// The effort levels, least first.
const EFFORTS: [Effort; 7] = [
    Effort::None,
    Effort::Minimal,
    Effort::Low,
    Effort::Medium,
    Effort::High,
    Effort::Xhigh,
    Effort::Max,
];

/// What a provider is asked next, after it refused how hard it was asked
/// to think ([`Asked`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Next {
    /// This level instead.
    Level(Effort),
    /// Nothing about thinking: the model takes none of it.
    Nothing,
}

/// Whether a refusal is about how hard the model was asked to think, as
/// the providers word it (`reasoning.effort`, `reasoning_effort`,
/// `thinkingLevel`, `thinking_level`, `thinkingBudget`).
fn about_thinking(why: &str) -> bool {
    let why = why.to_ascii_lowercase();
    [
        "effort",
        "thinkinglevel",
        "thinking_level",
        "thinking level",
        "thinkingbudget",
        "thinking_budget",
        "thinking budget",
    ]
    .iter()
    .any(|said| why.contains(said))
}

/// What to ask after a provider refused `asked` with `why`: the nearest of
/// the levels it says it takes, if it lists them (`Supported values are:
/// 'low', 'medium', 'high'`); nothing, if it says the model takes no such
/// parameter and names no level; or else the next level toward `medium`,
/// and nothing after `medium`. `None`: the refusal is not about thinking.
pub(crate) fn next_effort(why: &str, asked: Effort) -> Option<Next> {
    if !about_thinking(why) {
        return None;
    }
    let said = why.to_ascii_lowercase();
    let at = |level: Effort| EFFORTS.iter().position(|l| *l == level).unwrap_or(3);
    // The levels it names, after "supported values" if it says that; the
    // one refused is not among those it takes.
    let listed = said.find("supported values").map_or(said.as_str(), |from| {
        &said[from + "supported values".len()..]
    });
    let quoted = |level: Effort| {
        ['\'', '"', '`']
            .iter()
            .any(|q| listed.contains(&format!("{q}{}{q}", level.name())))
    };
    let taken: Vec<Effort> = EFFORTS
        .iter()
        .copied()
        .filter(|level| *level != asked && quoted(*level))
        .collect();
    if !taken.is_empty() {
        // The nearest, and of two as near the higher: more thought rather
        // than less.
        let nearest = taken
            .iter()
            .copied()
            .min_by_key(|level| (at(*level).abs_diff(at(asked)), usize::MAX - at(*level)));
        return nearest.map(Next::Level);
    }
    // A level named as a word of its own: the refusal is of that level,
    // not of the parameter.
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let names_level = EFFORTS.iter().any(|level| {
        said.match_indices(level.name()).any(|(from, name)| {
            !said[..from].chars().next_back().is_some_and(word)
                && !said[from + name.len()..].chars().next().is_some_and(word)
        })
    });
    let unknown = [
        "unsupported parameter",
        "unrecognized request argument",
        "unknown parameter",
        "unknown name",
        "not supported with this model",
        "not supported for this model",
        "not supported by this model",
        "does not support thinking",
        "does not support reasoning",
    ];
    if !names_level && unknown.iter().any(|u| said.contains(u)) {
        return Some(Next::Nothing);
    }
    Some(match asked {
        Effort::None => Next::Level(Effort::Minimal),
        Effort::Minimal => Next::Level(Effort::Low),
        Effort::Low => Next::Level(Effort::Medium),
        Effort::Max => Next::Level(Effort::Xhigh),
        Effort::Xhigh => Next::Level(Effort::High),
        Effort::High => Next::Level(Effort::Medium),
        Effort::Medium => Next::Nothing,
    })
}

/// How hard a provider's model is asked to think, once it refused what was
/// configured: models of one provider take different levels (one takes
/// `minimal`, another `none`, a third neither), and say so only by
/// refusing the request. The level a model was found to take is asked from
/// then on, so each is found once.
#[derive(Debug, Default)]
pub(crate) struct Asked(AtomicU8);

impl Asked {
    /// As configured.
    const AS_SAID: u8 = 0;
    /// Nothing about thinking.
    const NOTHING: u8 = u8::MAX;
    /// How often one call asks again at another level.
    pub(crate) const TRIES: usize = 4;

    /// What is asked now, for a model configured to think as `said`.
    pub(crate) fn thinking(&self, said: Option<Thinking>) -> Option<Thinking> {
        match self.0.load(Ordering::Relaxed) {
            Self::AS_SAID => said,
            Self::NOTHING => None,
            level => EFFORTS
                .get(usize::from(level) - 1)
                .copied()
                .map(Thinking::Effort),
        }
    }

    /// Whether `why` refuses what is asked now, and something else is asked
    /// from now on: the request is worth sending again.
    pub(crate) fn refused(&self, why: &str, said: Option<Thinking>) -> bool {
        if !why.starts_with("HTTP 400") && !why.starts_with("HTTP 422") {
            return false;
        }
        let asked = match self.thinking(said) {
            Some(Thinking::Effort(level)) => level,
            // A budget refused is the level nearest to it.
            Some(Thinking::Budget(tokens)) if about_thinking(why) => {
                let level = match tokens {
                    0 => Effort::None,
                    1..=2_047 => Effort::Low,
                    2_048..=11_999 => Effort::Medium,
                    _ => Effort::High,
                };
                self.set(Next::Level(level));
                return true;
            }
            _ => return false,
        };
        match next_effort(why, asked) {
            Some(next) => {
                self.set(next);
                true
            }
            None => false,
        }
    }

    fn set(&self, next: Next) {
        let stored = match next {
            Next::Nothing => Self::NOTHING,
            Next::Level(level) => EFFORTS
                .iter()
                .position(|l| *l == level)
                .map_or(Self::NOTHING, |at| at as u8 + 1),
        };
        self.0.store(stored, Ordering::Relaxed);
    }
}

/// Sends with `send`, and again while the provider refuses how hard the
/// model is asked to think and something else can be asked of it: `send`
/// builds its request from what `asked` says each time.
pub(crate) fn thought_as_taken<T>(
    asked: &Asked,
    said: Option<Thinking>,
    mut send: impl FnMut() -> Result<T, ModelError>,
) -> Result<T, ModelError> {
    let mut answered = send();
    for _ in 0..Asked::TRIES {
        match &answered {
            Err(ModelError::Refused(why)) if asked.refused(why, said) => answered = send(),
            _ => break,
        }
    }
    answered
}

/// An effort level as OpenAI names them (`none` to `max`): which of them a
/// model takes differs by model ([`openai_thinking`], [`Asked`]).
fn openai_effort(level: Effort) -> &'static str {
    level.name()
}

/// The effort levels a model of OpenAI's takes, where its model page says
/// (11 November 2026): GPT-6 Astra and GPT-6.1 Sol always reason (`low` to
/// `max`); GPT-6.1 Terra and GPT-6 Luna also take `none`; none of them
/// takes `minimal`. A model not known here is asked what is configured.
pub fn openai_levels(model: &str) -> Option<&'static [Effort]> {
    const REASONS: [Effort; 5] = [
        Effort::Low,
        Effort::Medium,
        Effort::High,
        Effort::Xhigh,
        Effort::Max,
    ];
    const OR_NOT: [Effort; 6] = [
        Effort::None,
        Effort::Low,
        Effort::Medium,
        Effort::High,
        Effort::Xhigh,
        Effort::Max,
    ];
    let name = crate::prices::normalized(model);
    let is = |family: &str| name == family || name.starts_with(&format!("{family}-"));
    if is("gpt-6-astra") || is("gpt-6.1-sol") {
        Some(&REASONS)
    } else if is("gpt-6.1-terra") || is("gpt-6-luna") {
        Some(&OR_NOT)
    } else {
        None
    }
}

/// A model's context window, in tokens, where its provider's pages say
/// (11 November 2026): Claude's models, and OpenAI's GPT-6 models
/// (1,050,000). `None` for any other: nothing is assumed of it.
pub fn window_of(model: &str) -> Option<u64> {
    let name = crate::prices::normalized(model);
    if name.starts_with("claude-") {
        Some(claude_traits(model).window)
    } else if openai_levels(model).is_some() {
        Some(1_050_000)
    } else {
        None
    }
}

/// How hard a model of OpenAI's is asked to think: the level configured,
/// or the nearest the model takes where that is known
/// ([`openai_levels`]), of two as near the higher.
pub(crate) fn openai_thinking(model: &str, thinking: Option<Thinking>) -> Option<Thinking> {
    let Thinking::Effort(level) = thinking? else {
        return thinking;
    };
    let Some(levels) = openai_levels(model) else {
        return thinking;
    };
    let at = |level: Effort| EFFORTS.iter().position(|l| *l == level).unwrap_or(3);
    levels
        .iter()
        .copied()
        .min_by_key(|taken| (at(*taken).abs_diff(at(level)), usize::MAX - at(*taken)))
        .map(Thinking::Effort)
}

/// What stands in a conversation for a picture a model is no longer shown.
pub const IMAGE_NOT_SHOWN: &str = "[a picture, no longer shown: take a new one to see]";

/// The pictures of a request, as the providers walk them: which are shown,
/// and the place of the next one.
pub(crate) struct Pictures<'a> {
    kept: &'a BTreeMap<String, String>,
    shown: BTreeSet<usize>,
    next: usize,
}

impl<'a> Pictures<'a> {
    pub(crate) fn of(messages: &[Message], kept: &'a BTreeMap<String, String>) -> Pictures<'a> {
        Pictures {
            kept,
            shown: images_shown(messages, kept),
            next: 0,
        }
    }

    /// The bytes of the next picture, `image`, if it is shown.
    pub(crate) fn next(&mut self, image: &Image) -> Option<&'a str> {
        let at = self.next;
        self.next += 1;
        if self.shown.contains(&at) {
            self.kept.get(&image.digest).map(String::as_str)
        } else {
            None
        }
    }
}

/// The most bytes of a model's answer read.
pub(crate) const MAX_ANSWER: u64 = 8 << 20;

/// The most bytes of one event of a streamed answer.
pub(crate) const MAX_EVENT: usize = 1 << 20;

/// `POST url` with `body`, asking for the answer as Server-Sent Events; an
/// error status is mapped as [`http_error`] maps it.
pub(crate) fn post_stream(
    client: &sion_http::Client,
    url: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> Result<sion_http::Response, ModelError> {
    let mut all = vec![
        ("Content-Type", "application/json"),
        ("Accept", "text/event-stream"),
    ];
    all.extend_from_slice(headers);
    let mut response = client
        .send("POST", url, &all, Some(body))
        .map_err(http_error)?;
    if !(200..300).contains(&response.status) {
        let retry_after = sion_http::retry_after(
            response.header("retry-after"),
            response.header("retry-after-ms"),
        );
        let mut start = Vec::new();
        let _ = response.body().take(2_048).read_to_end(&mut start);
        return Err(http_error(sion_http::HttpError::Status {
            status: response.status,
            body: String::from_utf8_lossy(&start).into_owned(),
            retry_after,
        }));
    }
    Ok(response)
}

/// Whether `response` streams events, rather than an answer whole.
pub(crate) fn streams(response: &sion_http::Response) -> bool {
    response.media_type().as_deref() == Some("text/event-stream")
}

/// An answer that came whole, although it was asked to stream: read in
/// full, with `read`, and told at once.
pub(crate) fn whole(
    response: sion_http::Response,
    read: fn(&Value) -> Result<Response, ModelError>,
    seen: &mut dyn FnMut(Delta) -> bool,
) -> Result<Response, ModelError> {
    let body = response.read_all(MAX_ANSWER).map_err(http_error)?;
    let value: Value = serde_json::from_slice(&body)
        .map_err(|e| ModelError::Protocol(format!("answer is not JSON: {e}")))?;
    let answer = read(&value)?;
    tell_whole(&answer, seen);
    Ok(answer)
}

/// A call's input from the JSON its arguments were streamed as: an empty
/// object for none, and what could not be parsed kept as it is.
fn input_of(arguments: &str) -> Value {
    if arguments.trim().is_empty() {
        return json!({});
    }
    serde_json::from_str(arguments).unwrap_or_else(|_| json!({ crate::model::UNPARSED: arguments }))
}

/// The error a stream reports partway, as the status it stands for: the
/// provider overloaded or failing is transient, the rest is not.
fn stream_error(error: &Value) -> ModelError {
    let kind = error["type"]
        .as_str()
        .or_else(|| error["code"].as_str())
        .unwrap_or("error");
    let message = error["message"].as_str().unwrap_or("no message");
    let said = format!("{kind} while streaming: {message}");
    match kind {
        _ if spent(&said) => ModelError::Limited {
            message: said,
            retry_after: REFUSED_REST.as_secs(),
        },
        "overloaded_error"
        | "rate_limit_error"
        | "api_error"
        | "timeout_error"
        | "server_error"
        | "internal_server_error"
        | "internal_error"
        | "service_unavailable_error"
        | "service_unavailable"
        | "server_is_overloaded"
        | "slow_down"
        | "rate_limit_exceeded"
        | "insufficient_capacity" => ModelError::Busy(said),
        _ if too_long(&said) => ModelError::TooLong(said),
        _ => ModelError::Refused(said),
    }
}

/// Whether a provider says the account's credit or quota is spent, which
/// asking again soon does not change: another key, or another model, may
/// answer meanwhile.
pub(crate) fn spent(body: &str) -> bool {
    let body = body.to_ascii_lowercase();
    [
        "insufficient_quota",
        "billing_hard_limit_reached",
        "billing_not_active",
        "credit balance is too low",
        "exceeded your current quota, please check your plan and billing",
    ]
    .iter()
    .any(|said| body.contains(said))
}

/// How long a provider asks to be left, in seconds, where it says so in
/// what it answers rather than in a `Retry-After` header: Google's
/// `"retryDelay": "33s"`, or OpenAI's `Please try again in 6.5s`. Rounded
/// up; a wait of milliseconds is a second.
pub(crate) fn asked_wait(body: &str) -> Option<u64> {
    let number = |text: &str| -> Option<(f64, usize)> {
        let end = text
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(text.len());
        text[..end].parse::<f64>().ok().map(|n| (n, end))
    };
    let seconds = |text: &str| -> Option<u64> {
        let (n, end) = number(text)?;
        let unit = text[end..].trim_start();
        let seconds = if unit.starts_with("ms") {
            n / 1_000.0
        } else if unit.starts_with('m') {
            n * 60.0
        } else if unit.starts_with('h') {
            n * 3_600.0
        } else {
            n
        };
        (seconds.is_finite() && seconds >= 0.0).then(|| (seconds.ceil() as u64).clamp(1, 86_400))
    };
    if let Some(at) = body.find("\"retryDelay\"") {
        let rest = body[at + "\"retryDelay\"".len()..].trim_start();
        let rest = rest.strip_prefix(':').unwrap_or(rest).trim_start();
        if let Some(found) = seconds(rest.strip_prefix('"').unwrap_or(rest)) {
            return Some(found);
        }
    }
    let lower = body.to_ascii_lowercase();
    let at = lower.find("try again in ")?;
    seconds(&lower[at + "try again in ".len()..])
}

/// Whether a provider's refusal says the conversation is longer than the
/// model's context window takes, as the providers word it.
pub(crate) fn too_long(body: &str) -> bool {
    let body = body.to_ascii_lowercase();
    [
        "prompt is too long",
        "context_length_exceeded",
        "context length",
        "context window",
        "maximum context",
        "exceeds the maximum number of tokens",
        "input token count",
        "too many tokens",
        "request_too_large",
        "reduce the length",
    ]
    .iter()
    .any(|said| body.contains(said))
}

pub(crate) fn http_error(error: sion_http::HttpError) -> ModelError {
    use sion_http::HttpError::*;
    match error {
        Status {
            status,
            body,
            retry_after,
        } => match status {
            408 | 409 | 425 | 429 | 500 | 502 | 503 | 504 | 529 => {
                // Credit or quota spent is not over in a minute; else as
                // long as the provider asks, in its header or its answer.
                let wait = if status == 429 && spent(&body) && asked_wait(&body).is_none() {
                    Some(retry_after.unwrap_or(0).max(REFUSED_REST.as_secs()))
                } else {
                    retry_after.or_else(|| asked_wait(&body))
                };
                match wait {
                    Some(retry_after) => ModelError::Limited {
                        message: format!("HTTP {status}: {body}"),
                        retry_after,
                    },
                    None => ModelError::Busy(format!("HTTP {status}: {body}")),
                }
            }
            400 | 413 | 422 if too_long(&body) => {
                ModelError::TooLong(format!("HTTP {status}: {body}"))
            }
            _ => ModelError::Refused(format!("HTTP {status}: {body}")),
        },
        Transport(e) => ModelError::Transport(e),
        Closed { code, reason } => {
            ModelError::Transport(format!("the connection was closed ({code}): {reason}"))
        }
        Refused(e) | Config(e) => ModelError::Refused(e),
        TooLarge(n) => ModelError::Protocol(format!("answer longer than {n} bytes")),
    }
}

/// How long a key rests that the provider limited (too many requests).
pub const LIMITED_REST: Duration = Duration::from_secs(60);

/// How long a key rests that the provider refused (not authorized, or out
/// of credit).
pub const REFUSED_REST: Duration = Duration::from_secs(3_600);

/// A provider's API keys, used in turn, one call after another: a key the
/// provider limits rests [`LIMITED_REST`], one it refuses [`REFUSED_REST`],
/// and the next key is tried at once, each once per call. Keys at rest are
/// tried last, the soonest awake first. Shown, it says only how many.
pub struct Keys {
    keys: Vec<String>,
    next: AtomicUsize,
    resting: Mutex<Vec<Option<Instant>>>,
}

impl std::fmt::Debug for Keys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Keys({})", self.keys.len())
    }
}

impl Keys {
    /// The keys of `list`: one, or several separated by commas.
    pub fn parse(list: &str) -> Keys {
        let keys: Vec<String> = list
            .split(',')
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .map(str::to_string)
            .collect();
        let resting = Mutex::new(vec![None; keys.len()]);
        Keys {
            keys,
            next: AtomicUsize::new(0),
            resting,
        }
    }

    /// How many keys there are.
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Calls `send` with a key, and with the next while the provider
    /// limits or refuses the one it was given and another may do better:
    /// what the last call returned. With no keys, `send` is given none.
    pub fn send<T>(
        &self,
        mut send: impl FnMut(Option<&str>) -> Result<T, ModelError>,
    ) -> Result<T, ModelError> {
        if self.keys.is_empty() {
            return send(None);
        }
        let now = Instant::now();
        let count = self.keys.len();
        let start = self.next.fetch_add(1, Ordering::Relaxed) % count;
        let resting = self
            .resting
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let mut order: Vec<usize> = (0..count).map(|i| (start + i) % count).collect();
        order.sort_by_key(|&i| resting[i].filter(|until| *until > now));
        let mut last = None;
        for at in order {
            match send(Some(&self.keys[at])) {
                Err(error) if count > 1 => match rest_for(&error) {
                    Some(rest) => {
                        if let Some(until) = self
                            .resting
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .get_mut(at)
                        {
                            *until = Some(Instant::now() + rest);
                        }
                        last = Some(error);
                    }
                    None => return Err(error),
                },
                answered => return answered,
            }
        }
        Err(last.unwrap_or_else(|| ModelError::Refused("no key was tried".into())))
    }
}

/// How long a key rests after `error`, if another key may do better: the
/// provider limited it, or refused it.
fn rest_for(error: &ModelError) -> Option<Duration> {
    match error {
        ModelError::Busy(why) if why.starts_with("HTTP 429") => Some(LIMITED_REST),
        // As long as the provider asked.
        ModelError::Limited {
            message,
            retry_after,
        } if message.starts_with("HTTP 429") => Some(Duration::from_secs((*retry_after).max(1))),
        ModelError::Refused(why)
            if ["HTTP 401", "HTTP 402", "HTTP 403"]
                .iter()
                .any(|status| why.starts_with(status)) =>
        {
            Some(REFUSED_REST)
        }
        _ => None,
    }
}

/// The Anthropic Messages API (`POST /v1/messages`, API version
/// `2026-10-21`: no sampling parameters are sent, and the top-level
/// `cache_control` asks for automatic prompt caching). Its thinking blocks,
/// signed, and redacted ones, are kept and given back as they came, as the
/// API asks within a task and recommends across tasks. An effort level is
/// adaptive thinking at that effort (`output_config.effort`; `none` turns
/// thinking off where a model allows it), and a budget is thinking with
/// that budget (`budget_tokens`, for models before Claude 4.6).
pub struct Anthropic {
    client: sion_http::Client,
    /// The API's base URL, `https://api.anthropic.com` by default.
    pub base: String,
    /// The model, for example `claude-opus-5-5`.
    pub model: String,
    keys: Keys,
    /// The `anthropic-version` header.
    pub version: String,
    /// Ask the API to cache the conversation's prefix.
    pub cache: bool,
    /// How hard it is asked to think, if said.
    pub thinking: Option<Thinking>,
    /// Whether the API is asked to let go of kept thinking that no longer
    /// fits what was sent before it, rather than refuse the request
    /// ([`CLAUDE_BINDING_BETA`]): off once the API has refused the asking.
    bind: std::sync::atomic::AtomicBool,
}

impl std::fmt::Debug for Anthropic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Anthropic({} at {})", self.model, self.base)
    }
}

impl Anthropic {
    /// A provider for `model` with the API key `key` (several, separated
    /// by commas, are used in turn: [`Keys`]).
    pub fn new(client: sion_http::Client, model: &str, key: &str) -> Anthropic {
        Anthropic {
            client,
            base: "https://api.anthropic.com".into(),
            model: model.into(),
            keys: Keys::parse(key),
            version: "2026-10-21".into(),
            cache: true,
            thinking: None,
            bind: std::sync::atomic::AtomicBool::new(true),
        }
    }

    /// Where its reasoning comes from, and the only reasoning it is given
    /// back: its kind, base URL and model.
    pub fn source(&self) -> String {
        format!(
            "anthropic {} {}",
            self.base.trim_end_matches('/'),
            self.model
        )
    }

    /// Whether the API is asked to let go of thinking that no longer fits.
    fn binds(&self) -> bool {
        self.bind.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Never asks that: for a platform that sends this request its own way,
    /// without the beta the asking needs.
    pub(crate) fn unbound(self) -> Anthropic {
        self.bind.store(false, std::sync::atomic::Ordering::Relaxed);
        self
    }

    /// The request body for `request`.
    pub fn body(&self, request: &Request) -> Value {
        self.body_from(request, &self.source())
    }

    /// The request body for `request`, giving back the reasoning kept from
    /// `from`: its own, or that of the platform it is reached through
    /// ([`crate::cloud::Vertex`]).
    pub(crate) fn body_from(&self, request: &Request, from: &str) -> Value {
        let mut pictures = Pictures::of(&request.messages, &request.images);
        let messages: Vec<Value> = request
            .messages
            .iter()
            .filter_map(|m| anthropic_message(m, &mut pictures, from))
            .collect();
        let tools: Vec<Value> = request
            .tools
            .iter()
            .map(|t| json!({"name": t.name, "description": t.description, "input_schema": t.input_schema}))
            .collect();
        let most = self.most_tokens(request.max_tokens);
        let mut body = json!({
            "model": self.model,
            "max_tokens": most,
            "system": request.system,
            "messages": messages,
        });
        if !tools.is_empty() {
            body["tools"] = json!(tools);
        }
        if self.cache {
            body["cache_control"] = json!({"type": "ephemeral"});
        }
        for (field, value) in claude_thinking(&self.model, self.thinking, most, self.binds()) {
            body[field] = value;
        }
        body
    }
}

/// How hard Claude thinks when asked to as `thinking` says: a budget at
/// least the API's least (1,024).
pub(crate) fn claude_thinks(thinking: Option<Thinking>) -> Option<Thinking> {
    thinking.map(|thinking| match thinking {
        Thinking::Budget(tokens) if tokens > 0 => Thinking::Budget(tokens.max(1_024)),
        other => other,
    })
}

/// How a Claude model's thinking is turned off, if it can be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaudeOff {
    /// `thinking.type: "disabled"`.
    Disabled,
    /// `"between_tools"`: no thinking before it answers.
    BetweenTools,
    /// It always thinks: the least is the lowest effort.
    Never,
}

/// What a Claude model takes of a request, by its name: the API answers
/// 400 to what a model does not (the table of
/// <https://platform.claude.com/docs/en/build-with-claude/thinking>, read
/// 2026-11-11).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClaudeTraits {
    /// It thinks when a request says nothing of thinking.
    pub thinks_unasked: bool,
    /// It takes `thinking.type: "adaptive"`.
    pub adaptive: bool,
    /// It takes `"enabled"` with `budget_tokens`.
    pub budget: bool,
    /// How its thinking is turned off.
    pub off: ClaudeOff,
    /// It takes `output_config.effort`.
    pub effort: bool,
    /// It takes the effort `xhigh`.
    pub xhigh: bool,
    /// It takes the effort `max`.
    pub max: bool,
    /// It checks that thinking given back was written over the same
    /// system prompt, tools and messages: thinking kept across a change to
    /// those is refused, unless the request asks for it to be let go.
    pub binds: bool,
    /// The most tokens it writes in an answer.
    pub most: u32,
    /// Its context window, in tokens.
    pub window: u64,
}

/// The traits of the Claude model `model` (a name as a provider, a cloud
/// platform or a router writes it). A model newer than those known is
/// taken for the newest of its line.
pub fn claude_traits(model: &str) -> ClaudeTraits {
    let name = crate::prices::normalized(model);
    let rest = name.strip_prefix("claude-").unwrap_or(&name);
    let mut parts = rest.split('-');
    let line = parts.next().unwrap_or_default();
    let number = |part: Option<&str>| {
        part.and_then(|p| p.parse::<u32>().ok())
            .filter(|n| *n < 100)
    };
    let major = number(parts.next());
    let minor = major.and_then(|_| number(parts.next())).unwrap_or(0);
    let version = (major.unwrap_or(u32::MAX), minor);
    let newest = ClaudeTraits {
        thinks_unasked: true,
        adaptive: true,
        budget: false,
        off: ClaudeOff::Never,
        effort: true,
        xhigh: true,
        max: true,
        binds: true,
        most: 128_000,
        window: 1_000_000,
    };
    // Before adaptive thinking: a budget, no effort, 200k tokens of context.
    let early = ClaudeTraits {
        thinks_unasked: false,
        adaptive: false,
        budget: true,
        off: ClaudeOff::Disabled,
        effort: false,
        xhigh: false,
        max: false,
        binds: false,
        most: 64_000,
        window: 200_000,
    };
    match line {
        "fable" | "mythos" if rest.contains("preview") => ClaudeTraits {
            budget: true,
            xhigh: false,
            binds: false,
            ..newest
        },
        "fable" | "mythos" if version < (5, 1) => ClaudeTraits {
            binds: false,
            ..newest
        },
        "fable" | "mythos" => newest,
        "opus" if version >= (5, 5) => newest,
        "opus" if version >= (5, 0) => ClaudeTraits {
            off: ClaudeOff::Disabled,
            binds: false,
            ..newest
        },
        "opus" if version >= (4, 7) => ClaudeTraits {
            thinks_unasked: false,
            off: ClaudeOff::Disabled,
            binds: false,
            ..newest
        },
        "opus" if version >= (4, 6) => ClaudeTraits {
            thinks_unasked: false,
            budget: true,
            off: ClaudeOff::Disabled,
            xhigh: false,
            binds: false,
            ..newest
        },
        "opus" if version >= (4, 5) => ClaudeTraits {
            effort: true,
            ..early
        },
        "opus" => ClaudeTraits {
            most: 32_000,
            ..early
        },
        "sonnet" if version >= (5, 5) => ClaudeTraits {
            off: ClaudeOff::BetweenTools,
            ..newest
        },
        "sonnet" if version >= (5, 0) => ClaudeTraits {
            off: ClaudeOff::Disabled,
            binds: false,
            ..newest
        },
        "sonnet" if version >= (4, 6) => ClaudeTraits {
            thinks_unasked: false,
            budget: true,
            off: ClaudeOff::Disabled,
            xhigh: false,
            binds: false,
            ..newest
        },
        "sonnet" => early,
        "haiku" if version >= (5, 5) => ClaudeTraits {
            off: ClaudeOff::Disabled,
            ..newest
        },
        "haiku" => early,
        // Claude 3 and before, named with the number first.
        _ if line.chars().all(|c| c.is_ascii_digit()) && !line.is_empty() => ClaudeTraits {
            most: 8_192,
            ..early
        },
        _ => newest,
    }
}

/// The beta that lets a request say what the API does with kept thinking
/// that no longer fits what was sent before it.
pub const CLAUDE_BINDING_BETA: &str = "thinking-binding-controls-2026-08-01";

/// The fields of a request to the Claude model `model` that say how hard
/// it thinks (`thinking`, and `output_config` for an effort level) when it
/// may write `most` tokens in all, as that model takes them
/// ([`claude_traits`]): an effort level a model does not have is the next
/// one down; thinking that cannot be turned off is asked for at the lowest
/// effort; a budget, where a model takes none, is the effort nearest it.
/// With `bind`, a model that checks kept thinking against what was sent
/// before it is asked to let go of thinking that no longer fits, rather
/// than refuse the request (which needs [`CLAUDE_BINDING_BETA`]).
pub(crate) fn claude_thinking(
    model: &str,
    thinking: Option<Thinking>,
    most: u32,
    bind: bool,
) -> Vec<(&'static str, Value)> {
    let traits = claude_traits(model);
    let adaptive = || {
        if bind && traits.binds {
            json!({"type": "adaptive", "block_binding": {"prefix_mismatch_behavior": "drop_block"}})
        } else {
            json!({"type": "adaptive"})
        }
    };
    let effort = |level: Effort| {
        let name = match level {
            Effort::None | Effort::Minimal | Effort::Low => "low",
            Effort::Medium => "medium",
            Effort::High => "high",
            Effort::Xhigh if traits.xhigh => "xhigh",
            Effort::Xhigh => "high",
            Effort::Max if traits.max => "max",
            Effort::Max if traits.xhigh => "xhigh",
            Effort::Max => "high",
        };
        json!({"effort": name})
    };
    let budget = |tokens: u32| {
        // Less than all it may write, as the API asks.
        json!({"type": "enabled", "budget_tokens": tokens.max(1_024).min(most.saturating_sub(1))})
    };
    let with_effort = |level: Effort| {
        let mut fields = vec![("thinking", adaptive())];
        if traits.effort {
            fields.push(("output_config", effort(level)));
        }
        fields
    };
    match claude_thinks(thinking) {
        // Nothing said: the model's own way, and, where it checks kept
        // thinking, the asking that it let go of what no longer fits.
        None if bind && traits.binds && traits.thinks_unasked => vec![("thinking", adaptive())],
        None => Vec::new(),
        Some(Thinking::Effort(Effort::None) | Thinking::Budget(0)) => match traits.off {
            ClaudeOff::Disabled => vec![("thinking", json!({"type": "disabled"}))],
            ClaudeOff::BetweenTools => vec![("thinking", json!({"type": "between_tools"}))],
            ClaudeOff::Never => with_effort(Effort::Low),
        },
        Some(Thinking::Effort(level)) if traits.adaptive => with_effort(level),
        Some(Thinking::Effort(level)) => {
            let mut fields = vec![("thinking", budget(Thinking::Effort(level).room()))];
            if traits.effort {
                // Opus 4.5 takes low to high.
                fields.push(("output_config", effort(level.min(Effort::High))));
            }
            fields
        }
        Some(Thinking::Budget(tokens)) if traits.budget => vec![("thinking", budget(tokens))],
        Some(Thinking::Budget(tokens)) => with_effort(match tokens {
            0..=4_096 => Effort::Low,
            4_097..=12_000 => Effort::Medium,
            12_001..=24_000 => Effort::High,
            24_001..=48_000 => Effort::Xhigh,
            _ => Effort::Max,
        }),
    }
}

/// A message as the Messages API takes it; none if nothing of it is left to
/// send (thinking of another model's only, or empty text: the API refuses
/// an empty text block and a message without content).
fn anthropic_message(message: &Message, pictures: &mut Pictures<'_>, from: &str) -> Option<Value> {
    let content: Vec<Value> = message
        .content
        .iter()
        .filter_map(|b| {
            Some(match b {
            Block::Text { text } if text.trim().is_empty() => return None,
            Block::Text { text } => json!({"type": "text", "text": text}),
            // Its thinking, as it came: only its own.
            Block::Reasoning { .. } => kept_from(b, from)?.clone(),
            Block::ToolUse {
                id, name, input, ..
            } => {
                json!({"type": "tool_use", "id": id, "name": name, "input": input})
            }
            Block::ToolResult {
                id,
                content,
                is_error,
                images,
            } if images.is_empty() => {
                let content = if content.is_empty() { "(no output)" } else { content.as_str() };
                json!({"type": "tool_result", "tool_use_id": id, "content": content, "is_error": is_error})
            }
            // A result with pictures is a list of blocks: its text, then
            // each picture, or a line for one no longer shown.
            Block::ToolResult {
                id,
                content,
                is_error,
                images,
            } => {
                let mut parts = Vec::new();
                if !content.is_empty() {
                    parts.push(json!({"type": "text", "text": content}));
                }
                for image in images {
                    parts.push(match pictures.next(image) {
                        Some(data) => json!({"type": "image", "source": {
                            "type": "base64", "media_type": image.media_type, "data": data}}),
                        None => json!({"type": "text", "text": IMAGE_NOT_SHOWN}),
                    });
                }
                json!({"type": "tool_result", "tool_use_id": id, "content": parts, "is_error": is_error})
            }
            Block::Image { image } => match pictures.next(image) {
                Some(data) => json!({"type": "image", "source": {
                    "type": "base64", "media_type": image.media_type, "data": data}}),
                None => json!({"type": "text", "text": IMAGE_NOT_SHOWN}),
            },
            })
        })
        .collect();
    if content.is_empty() {
        return None;
    }
    Some(
        json!({"role": match message.role { Role::User => "user", Role::Assistant => "assistant" }, "content": content}),
    )
}

/// A thinking block of Anthropic's, kept if it can be given back: a
/// thinking block with its signature (one cut off before it was signed
/// cannot), or a redacted one. Marked as its provider's when the answer is
/// returned ([`reasoning_from`]).
fn anthropic_thinking(block: &Value) -> Option<Block> {
    let whole = match block["type"].as_str() {
        Some("thinking") => block["signature"].as_str().is_some_and(|s| !s.is_empty()),
        Some("redacted_thinking") => block["data"].as_str().is_some_and(|d| !d.is_empty()),
        _ => false,
    };
    whole.then(|| Block::Reasoning {
        from: String::new(),
        data: block.clone(),
    })
}

/// Reads an Anthropic Messages API answer: its text, its calls and its
/// thinking ([`anthropic_thinking`]), in order.
pub fn anthropic_response(answer: &Value) -> Result<Response, ModelError> {
    let blocks = answer["content"]
        .as_array()
        .ok_or_else(|| ModelError::Protocol("no content in the answer".into()))?;
    let mut content = Vec::new();
    for block in blocks {
        match block["type"].as_str() {
            Some("text") => content.push(Block::text(block["text"].as_str().unwrap_or_default())),
            Some("tool_use") => content.push(Block::ToolUse {
                id: block["id"].as_str().unwrap_or_default().to_string(),
                name: block["name"].as_str().unwrap_or_default().to_string(),
                input: block["input"].clone(),
                signature: None,
            }),
            // Its thinking, to be given back; other blocks are not part of
            // the conversation this runtime keeps.
            _ => content.extend(anthropic_thinking(block)),
        }
    }
    Ok(Response {
        content,
        stop: anthropic_stop(answer["stop_reason"].as_str()),
        usage: anthropic_usage(&answer["usage"]),
        model: answer["model"].as_str().unwrap_or_default().to_string(),
        forgot_reasoning: let_go(&answer["input_transformations"]),
    })
}

/// Whether the API let go of kept thinking because what was sent before it
/// had changed (`input_transformations`, under [`CLAUDE_BINDING_BETA`]):
/// that thinking, and all after it, is of no use to send again.
fn let_go(transformations: &Value) -> bool {
    transformations.as_array().is_some_and(|all| {
        all.iter()
            .any(|t| t["type"] == "thinking_dropped" && t["reason"] == "prefix_binding_mismatch")
    })
}

fn anthropic_stop(reason: Option<&str>) -> Stop {
    match reason {
        Some("end_turn") | Some("stop_sequence") => Stop::EndTurn,
        Some("tool_use") => Stop::ToolUse,
        Some("max_tokens") => Stop::MaxTokens,
        Some("refusal") => Stop::Refusal,
        Some("pause_turn") => Stop::PauseTurn,
        Some("model_context_window_exceeded") => Stop::ContextFull,
        other => Stop::Other(other.unwrap_or("none").to_string()),
    }
}

/// A block of a streamed Anthropic answer, as it is built.
enum Building {
    Text(String),
    Tool {
        id: String,
        name: String,
        json: String,
    },
    /// Thinking, and its signature, which comes last.
    Thinking {
        thinking: String,
        signature: String,
    },
    /// Redacted thinking, which comes whole.
    Redacted(Value),
    /// The rest, not kept (as [`anthropic_response`] does not).
    Other,
}

/// Reads a streamed Anthropic Messages API answer (`"stream": true`) from
/// its events, telling `seen` its text and its calls as they come: the
/// answer is what [`anthropic_response`] reads from the same answer whole.
/// Thinking is not told; it is kept, signed.
pub fn anthropic_stream<R: Read>(
    events: &mut Events<R>,
    seen: &mut dyn FnMut(Delta) -> bool,
) -> Result<Response, ModelError> {
    let mut blocks: BTreeMap<u64, Building> = BTreeMap::new();
    let mut texts = 0usize;
    let mut model = String::new();
    let mut usage = Usage::default();
    let mut stop: Option<String> = None;
    let mut ended = false;
    let mut forgot = false;
    // Whether the watcher cut the answer off.
    let mut cut = false;
    while let Some(event) = events.next_event() {
        let event = event.map_err(ModelError::Transport)?;
        let Ok(data) = serde_json::from_str::<Value>(&event.data) else {
            continue;
        };
        let index = data["index"].as_u64().unwrap_or(0);
        match data["type"].as_str().unwrap_or(event.name.as_str()) {
            "message_start" => {
                let message = &data["message"];
                model = message["model"].as_str().unwrap_or_default().to_string();
                usage = anthropic_usage(&message["usage"]);
                forgot |= let_go(&message["input_transformations"]);
            }
            "content_block_start" => {
                let block = &data["content_block"];
                let building = match block["type"].as_str() {
                    Some("text") => {
                        let text = block["text"].as_str().unwrap_or_default().to_string();
                        // Told as the answer's text reads: blocks joined by
                        // line feeds.
                        let told = if texts == 0 {
                            text.clone()
                        } else {
                            format!("\n{text}")
                        };
                        texts += 1;
                        if !told.is_empty() {
                            cut = !seen(Delta::Text(told));
                        }
                        Building::Text(text)
                    }
                    Some("tool_use") => {
                        let name = block["name"].as_str().unwrap_or_default().to_string();
                        cut = !seen(Delta::Call(name.clone()));
                        Building::Tool {
                            id: block["id"].as_str().unwrap_or_default().to_string(),
                            name,
                            json: String::new(),
                        }
                    }
                    Some("thinking") => Building::Thinking {
                        thinking: block["thinking"].as_str().unwrap_or_default().to_string(),
                        signature: block["signature"].as_str().unwrap_or_default().to_string(),
                    },
                    Some("redacted_thinking") => Building::Redacted(block.clone()),
                    _ => Building::Other,
                };
                blocks.insert(index, building);
            }
            "content_block_delta" => {
                let delta = &data["delta"];
                match (blocks.get_mut(&index), delta["type"].as_str()) {
                    (Some(Building::Text(text)), Some("text_delta")) => {
                        let more = delta["text"].as_str().unwrap_or_default();
                        text.push_str(more);
                        if !more.is_empty() {
                            cut = !seen(Delta::Text(more.to_string()));
                        }
                    }
                    (Some(Building::Tool { json, .. }), Some("input_json_delta")) => {
                        json.push_str(delta["partial_json"].as_str().unwrap_or_default());
                    }
                    (Some(Building::Thinking { thinking, .. }), Some("thinking_delta")) => {
                        thinking.push_str(delta["thinking"].as_str().unwrap_or_default());
                    }
                    (Some(Building::Thinking { signature, .. }), Some("signature_delta")) => {
                        signature.push_str(delta["signature"].as_str().unwrap_or_default());
                    }
                    _ => {}
                }
            }
            "message_delta" => {
                if let Some(reason) = data["delta"]["stop_reason"].as_str() {
                    stop = Some(reason.to_string());
                }
                forgot |= let_go(&data["input_transformations"]);
                let more = &data["usage"];
                if let Some(output) = more["output_tokens"].as_u64() {
                    usage.output = output;
                }
                if let Some(input) = more["input_tokens"].as_u64() {
                    let read = more["cache_read_input_tokens"]
                        .as_u64()
                        .unwrap_or(usage.cache_read);
                    let write = more["cache_creation_input_tokens"]
                        .as_u64()
                        .unwrap_or(usage.cache_write);
                    usage.input = input + read + write;
                    usage.cache_read = read;
                    usage.cache_write = write;
                }
            }
            "message_stop" => {
                ended = true;
                break;
            }
            "error" => return Err(stream_error(&data["error"])),
            // `ping`, `content_block_stop`, and what a later version adds.
            _ => {}
        }
        // Cut off: nothing more is read, and the connection closes.
        if cut {
            break;
        }
    }
    if cut {
        let texts = blocks.into_values().filter_map(|block| match block {
            Building::Text(text) => Some(text),
            _ => None,
        });
        return Ok(cut_off(texts, usage, model));
    }
    if !ended {
        return Err(ModelError::Transport(
            "the answer stopped before its end".into(),
        ));
    }
    let content = blocks
        .into_values()
        .filter_map(|block| match block {
            Building::Text(text) => Some(Block::text(text)),
            Building::Tool { id, name, json } => Some(Block::ToolUse {
                id,
                name,
                input: input_of(&json),
                signature: None,
            }),
            Building::Thinking {
                thinking,
                signature,
            } => anthropic_thinking(
                &json!({"type": "thinking", "thinking": thinking, "signature": signature}),
            ),
            Building::Redacted(block) => anthropic_thinking(&block),
            Building::Other => None,
        })
        .collect();
    Ok(Response {
        content,
        stop: anthropic_stop(stop.as_deref()),
        usage,
        model,
        forgot_reasoning: forgot,
    })
}

impl Anthropic {
    fn url(&self) -> String {
        format!("{}/v1/messages", self.base.trim_end_matches('/'))
    }
}

impl Anthropic {
    /// The `anthropic-beta` header `body` needs, if any.
    fn beta(body: &Value) -> Option<&'static str> {
        body["thinking"]["block_binding"]
            .is_object()
            .then_some(CLAUDE_BINDING_BETA)
    }

    /// Sends what `send` sends; if the API refuses the asking that it let go
    /// of thinking that no longer fits (an API or a platform without that
    /// beta), sends again without it, and no longer asks.
    fn bound<T>(&self, mut send: impl FnMut() -> Result<T, ModelError>) -> Result<T, ModelError> {
        match send() {
            Err(ModelError::Refused(why))
                if self.binds()
                    && why.starts_with("HTTP 400")
                    && (why.contains("block_binding") || why.contains("anthropic-beta")) =>
            {
                self.bind.store(false, std::sync::atomic::Ordering::Relaxed);
                send()
            }
            answered => answered,
        }
    }

    fn complete_once(&self, request: &Request) -> Result<Response, ModelError> {
        self.bound(|| {
            let body = self.body(request);
            let beta = Self::beta(&body);
            let body =
                serde_json::to_vec(&body).map_err(|e| ModelError::Protocol(e.to_string()))?;
            let answer = self.keys.send(|key| {
                let mut headers = vec![
                    ("x-api-key", key.unwrap_or_default()),
                    ("anthropic-version", self.version.as_str()),
                ];
                headers.extend(beta.map(|beta| ("anthropic-beta", beta)));
                self.client
                    .post_json(&self.url(), &headers, &body, MAX_ANSWER)
                    .map_err(http_error)
            })?;
            let value: Value = serde_json::from_slice(&answer.body)
                .map_err(|e| ModelError::Protocol(format!("answer is not JSON: {e}")))?;
            anthropic_response(&value)
        })
    }

    fn stream_once(
        &self,
        request: &Request,
        seen: &mut dyn FnMut(Delta) -> bool,
    ) -> Result<Response, ModelError> {
        let mut response = self.bound(|| {
            let mut body = self.body(request);
            body["stream"] = json!(true);
            let beta = Self::beta(&body);
            let body =
                serde_json::to_vec(&body).map_err(|e| ModelError::Protocol(e.to_string()))?;
            self.keys.send(|key| {
                let mut headers = vec![
                    ("x-api-key", key.unwrap_or_default()),
                    ("anthropic-version", self.version.as_str()),
                ];
                headers.extend(beta.map(|beta| ("anthropic-beta", beta)));
                post_stream(&self.client, &self.url(), &headers, &body)
            })
        })?;
        if !streams(&response) {
            return whole(response, anthropic_response, seen);
        }
        anthropic_stream(&mut Events::new(response.body(), MAX_EVENT), seen)
    }
}

/// The most tokens the Claude model `model` may write when it is asked for
/// at most `max_tokens` and to think as `thinking` says: room for its
/// thinking besides ([`most_tokens`]), also when nothing is said of
/// thinking and the model thinks anyway, up to what the model writes at
/// most.
pub(crate) fn claude_most(model: &str, thinking: Option<Thinking>, max_tokens: u32) -> u32 {
    let traits = claude_traits(model);
    let thinking = match claude_thinks(thinking) {
        None if traits.thinks_unasked => Some(Thinking::Effort(Effort::High)),
        // Thinking that cannot be turned off runs at the lowest effort.
        Some(Thinking::Effort(Effort::None) | Thinking::Budget(0))
            if traits.off == ClaudeOff::Never =>
        {
            Some(Thinking::Effort(Effort::Low))
        }
        other => other,
    };
    most_tokens(thinking, max_tokens).min(traits.most)
}

impl Model for Anthropic {
    fn id(&self) -> &str {
        &self.model
    }

    fn complete(&self, request: &Request) -> Result<Response, ModelError> {
        kept_or_not(request, &self.source(), |request| {
            self.complete_once(request)
        })
    }

    fn stream(
        &self,
        request: &Request,
        seen: &mut dyn FnMut(Delta) -> bool,
    ) -> Result<Response, ModelError> {
        kept_or_not(request, &self.source(), |request| {
            self.stream_once(request, seen)
        })
    }

    fn most_tokens(&self, max_tokens: u32) -> u32 {
        claude_most(&self.model, self.thinking, max_tokens)
    }
}

/// Any API that speaks OpenAI's chat completions with tools (`POST
/// {base}/chat/completions`), local servers included. The reasoning text a
/// server returns (`reasoning_content`, or `reasoning`) is kept and given
/// back, under the name it came by, within the task it was for, as servers
/// that think between tool calls ask. An effort level is
/// `reasoning_effort`, the nearest the model takes once it refuses the one
/// configured ([`Asked`]); the most it may write is `max_tokens`, or
/// `max_completion_tokens` once the server asks for that instead.
pub struct OpenAiCompatible {
    client: sion_http::Client,
    /// The API's base URL, for example `https://api.openai.com/v1`.
    pub base: String,
    /// The model.
    pub model: String,
    keys: Keys,
    /// How hard it is asked to think, if said.
    pub thinking: Option<Thinking>,
    /// Whether the server takes `max_completion_tokens` rather than
    /// `max_tokens` (OpenAI's reasoning models): learned when it refuses
    /// the one.
    completion_tokens: AtomicBool,
    /// How hard it is asked to think, once it refused what is configured.
    asked: Asked,
    /// The header the key goes in as it is (Azure's `api-key`); none: as
    /// `Authorization: Bearer <key>`.
    pub key_header: Option<String>,
}

impl std::fmt::Debug for OpenAiCompatible {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "OpenAiCompatible({} at {})", self.model, self.base)
    }
}

impl OpenAiCompatible {
    /// A provider at `base` for `model`, with an optional bearer key
    /// (several, separated by commas, are used in turn: [`Keys`]).
    pub fn new(client: sion_http::Client, base: &str, model: &str, key: Option<&str>) -> Self {
        OpenAiCompatible {
            client,
            base: base.into(),
            model: model.into(),
            keys: Keys::parse(key.unwrap_or_default()),
            thinking: None,
            completion_tokens: AtomicBool::new(false),
            asked: Asked::default(),
            key_header: None,
        }
    }

    /// Where its reasoning comes from, and the only reasoning it is given
    /// back: its kind, base URL and model.
    pub fn source(&self) -> String {
        format!(
            "openai-chat {} {}",
            self.base.trim_end_matches('/'),
            self.model
        )
    }

    /// The request body for `request`.
    pub fn body(&self, request: &Request) -> Value {
        let messages = openai_messages(
            &request.system,
            &request.messages,
            &request.images,
            &self.source(),
        );
        let tools = openai_tools(&request.tools);
        let most = self.most_tokens(request.max_tokens);
        let mut body = json!({"model": self.model, "messages": messages});
        if self.completion_tokens.load(Ordering::Relaxed) {
            body["max_completion_tokens"] = json!(most);
        } else {
            body["max_tokens"] = json!(most);
        }
        if !tools.is_empty() {
            body["tools"] = json!(tools);
        }
        self.effort_into(&mut body);
        body
    }

    /// How hard it is configured to think, as its model takes levels.
    fn said(&self) -> Option<Thinking> {
        openai_thinking(&self.model, self.thinking)
    }

    /// Says in `body` how hard the model thinks, as it is asked now.
    fn effort_into(&self, body: &mut Value) {
        match self.asked.thinking(self.said()) {
            Some(Thinking::Effort(level)) => {
                body["reasoning_effort"] = json!(openai_effort(level));
            }
            _ => {
                if let Some(fields) = body.as_object_mut() {
                    fields.remove("reasoning_effort");
                }
            }
        }
    }

    /// Whether the server refused how hard the model was asked to think:
    /// from now on it is asked the nearest it takes, and `body` is changed
    /// to say it.
    fn thinks_otherwise(&self, why: &str, body: &mut Value) -> bool {
        if body.get("reasoning_effort").is_none() || !self.asked.refused(why, self.said()) {
            return false;
        }
        self.effort_into(body);
        true
    }
}

/// Tools as OpenAI's chat completions define them: `{"type": "function",
/// "function": {name, description, parameters}}`.
pub fn openai_tools(tools: &[crate::model::ToolSpec]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            json!({"type": "function", "function": {
                "name": t.name,
                "description": t.description,
                "parameters": t.input_schema,
            }})
        })
        .collect()
}

/// A conversation in OpenAI's chat format: the system prompt, then each
/// message, with tool calls on assistant messages and one `tool` message per
/// result. A `tool` message holds text only: the pictures results returned
/// follow them in a user message, those `images` still keeps and among the
/// last shown. The reasoning kept from `from` ([`OpenAiCompatible::source`])
/// rides on the model's messages of the task at hand ([`task_start`]), under
/// the name the server gave it; none, for a `from` nothing came from.
pub fn openai_messages(
    system: &str,
    conversation: &[Message],
    images: &BTreeMap<String, String>,
    from: &str,
) -> Vec<Value> {
    let mut pictures = Pictures::of(conversation, images);
    let mut messages = vec![json!({"role": "system", "content": system})];
    let task = task_start(conversation);
    for (at, message) in conversation.iter().enumerate() {
        match message.role {
            Role::Assistant => {
                let calls: Vec<Value> = message
                    .tool_uses()
                    .map(|(id, name, input)| {
                        json!({"id": id, "type": "function", "function": {"name": name, "arguments": input.to_string()}})
                    })
                    .collect();
                let mut m = json!({"role": "assistant", "content": message.text()});
                if !calls.is_empty() {
                    m["tool_calls"] = json!(calls);
                }
                if at >= task {
                    for kept in message.content.iter().filter_map(|b| kept_from(b, from)) {
                        for (name, value) in kept.as_object().into_iter().flatten() {
                            m[name.as_str()] = value.clone();
                        }
                    }
                }
                messages.push(m);
            }
            Role::User => {
                let mut shown: Vec<Value> = Vec::new();
                // The person's own pictures, with their text.
                let mut own: Vec<Value> = Vec::new();
                for block in &message.content {
                    if let Block::Image { image } = block {
                        own.push(match pictures.next(image) {
                            Some(data) => json!({"type": "image_url", "image_url": {
                                "url": format!("data:{};base64,{data}", image.media_type)}}),
                            None => json!({"type": "text", "text": IMAGE_NOT_SHOWN}),
                        });
                    }
                    if let Block::ToolResult {
                        id,
                        content,
                        is_error,
                        images,
                    } = block
                    {
                        let mut content = if *is_error {
                            format!("error: {content}")
                        } else {
                            content.clone()
                        };
                        for image in images {
                            match pictures.next(image) {
                                Some(data) => {
                                    content.push_str("\n[the picture it returned follows]");
                                    shown.push(json!({"type": "image_url", "image_url": {
                                        "url": format!("data:{};base64,{data}", image.media_type)}}));
                                }
                                None => {
                                    content.push('\n');
                                    content.push_str(IMAGE_NOT_SHOWN);
                                }
                            }
                        }
                        messages
                            .push(json!({"role": "tool", "tool_call_id": id, "content": content}));
                    }
                }
                if !shown.is_empty() {
                    let mut parts = vec![json!({"type": "text",
                        "text": "The pictures the tool calls above returned:"})];
                    parts.extend(shown);
                    messages.push(json!({"role": "user", "content": parts}));
                }
                let text = message.text();
                if !own.is_empty() {
                    let mut parts = Vec::new();
                    if !text.is_empty() {
                        parts.push(json!({"type": "text", "text": text}));
                    }
                    parts.extend(own);
                    messages.push(json!({"role": "user", "content": parts}));
                } else if !text.is_empty() {
                    messages.push(json!({"role": "user", "content": text}));
                }
            }
        }
    }
    messages
}

/// The names a chat completions server gives what its model thought by,
/// in the order they are looked for.
const REASONING_FIELDS: [&str; 2] = ["reasoning_content", "reasoning"];

/// The reasoning text a chat completion's message (or a streamed chunk's
/// delta) carries, with the name it came by.
fn chat_reasoning(message: &Value) -> Option<(&'static str, &str)> {
    REASONING_FIELDS.iter().find_map(|&name| {
        message[name]
            .as_str()
            .filter(|text| !text.is_empty())
            .map(|text| (name, text))
    })
}

/// Reads an OpenAI-compatible chat completion: the reasoning it returned,
/// kept, then its text, then its calls.
pub fn openai_response(answer: &Value) -> Result<Response, ModelError> {
    let choice = &answer["choices"][0];
    let message = &choice["message"];
    if message.is_null() {
        return Err(ModelError::Protocol("no choices in the answer".into()));
    }
    let mut content = Vec::new();
    if let Some((name, text)) = chat_reasoning(message) {
        content.push(Block::Reasoning {
            from: String::new(),
            data: json!({ name: text }),
        });
    }
    if let Some(text) = message["content"].as_str() {
        if !text.is_empty() {
            content.push(Block::text(text));
        }
    }
    // What it says in declining, in place of an answer.
    let refusal = message["refusal"].as_str().filter(|said| !said.is_empty());
    if let Some(said) = refusal {
        content.push(Block::text(said));
    }
    let mut called = false;
    for call in message["tool_calls"].as_array().into_iter().flatten() {
        called = true;
        content.push(Block::ToolUse {
            id: call["id"].as_str().unwrap_or_default().to_string(),
            name: call["function"]["name"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            input: input_of(call["function"]["arguments"].as_str().unwrap_or_default()),
            signature: None,
        });
    }
    Ok(Response {
        content,
        stop: openai_stop(choice["finish_reason"].as_str(), called, refusal.is_some()),
        usage: openai_usage(&answer["usage"]),
        model: answer["model"].as_str().unwrap_or_default().to_string(),
        forgot_reasoning: false,
    })
}

/// Why a chat completion stopped: cut off (`length`) whatever it had
/// begun, calls included; declined, if it says so; with calls, to have
/// them made, whatever the server names that (some say `stop`).
fn openai_stop(reason: Option<&str>, called: bool, refused: bool) -> Stop {
    match reason {
        Some("length") => Stop::MaxTokens,
        Some("content_filter") => Stop::Refusal,
        _ if refused => Stop::Refusal,
        _ if called => Stop::ToolUse,
        Some("stop") => Stop::EndTurn,
        Some("tool_calls") | Some("function_call") => Stop::ToolUse,
        other => Stop::Other(other.unwrap_or("none").to_string()),
    }
}

/// A call of a streamed chat completion, as it is built.
#[derive(Default)]
struct Calling {
    id: String,
    name: String,
    arguments: String,
}

/// Reads a streamed chat completion (`"stream": true`) from its events,
/// telling `seen` its text and its calls as they come: the answer is what
/// [`openai_response`] reads from the same completion whole. The usage is
/// the last chunk's, when `stream_options.include_usage` asked for it. The
/// stream ends at `data: [DONE]`; one that ends without it is whole only if
/// it said why the model stopped.
pub fn openai_stream<R: Read>(
    events: &mut Events<R>,
    seen: &mut dyn FnMut(Delta) -> bool,
) -> Result<Response, ModelError> {
    let mut text = String::new();
    // What it thought, and the name the server gives it.
    let mut thought: Option<(&'static str, String)> = None;
    let mut calls: BTreeMap<u64, Calling> = BTreeMap::new();
    let mut finish: Option<String> = None;
    // What it says in declining.
    let mut refusal = String::new();
    let mut usage = Usage::default();
    let mut model = String::new();
    let mut done = false;
    // Whether the watcher cut the answer off.
    let mut cut = false;
    while let Some(event) = events.next_event() {
        let event = event.map_err(ModelError::Transport)?;
        if event.data.trim() == "[DONE]" {
            done = true;
            break;
        }
        let chunk: Value = serde_json::from_str(&event.data)
            .map_err(|e| ModelError::Protocol(format!("a streamed chunk is not JSON: {e}")))?;
        if chunk["error"].is_object() {
            return Err(stream_error(&chunk["error"]));
        }
        if model.is_empty() {
            model = chunk["model"].as_str().unwrap_or_default().to_string();
        }
        if chunk["usage"].is_object() {
            usage = openai_usage(&chunk["usage"]);
        }
        for choice in chunk["choices"].as_array().into_iter().flatten() {
            if choice["index"].as_u64().unwrap_or(0) != 0 {
                continue;
            }
            let delta = &choice["delta"];
            if let Some((name, more)) = chat_reasoning(delta) {
                let (_, so_far) = thought.get_or_insert((name, String::new()));
                so_far.push_str(more);
            }
            if let Some(more) = delta["content"].as_str() {
                if !more.is_empty() {
                    text.push_str(more);
                    cut = cut || !seen(Delta::Text(more.to_string()));
                }
            }
            if let Some(more) = delta["refusal"].as_str() {
                if !more.is_empty() {
                    refusal.push_str(more);
                    cut = cut || !seen(Delta::Text(more.to_string()));
                }
            }
            for call in delta["tool_calls"].as_array().into_iter().flatten() {
                let at = call["index"].as_u64().unwrap_or(0);
                let building = calls.entry(at).or_default();
                if let Some(id) = call["id"].as_str().filter(|id| !id.is_empty()) {
                    building.id = id.to_string();
                }
                if let Some(name) = call["function"]["name"].as_str().filter(|n| !n.is_empty()) {
                    if building.name.is_empty() {
                        cut = cut || !seen(Delta::Call(name.to_string()));
                    }
                    building.name.push_str(name);
                }
                if let Some(arguments) = call["function"]["arguments"].as_str() {
                    building.arguments.push_str(arguments);
                }
            }
            if let Some(reason) = choice["finish_reason"].as_str() {
                finish = Some(reason.to_string());
            }
        }
        // Cut off: nothing more is read, and the connection closes.
        if cut {
            break;
        }
    }
    if cut {
        return Ok(cut_off([text, refusal], usage, model));
    }
    if !done && finish.is_none() {
        return Err(ModelError::Transport(
            "the answer stopped before its end".into(),
        ));
    }
    let mut content = Vec::new();
    if let Some((name, thought)) = thought {
        content.push(Block::Reasoning {
            from: String::new(),
            data: json!({ name: thought }),
        });
    }
    if !text.is_empty() {
        content.push(Block::text(text));
    }
    let refused = !refusal.is_empty();
    if refused {
        content.push(Block::text(refusal));
    }
    let called = !calls.is_empty();
    for call in calls.into_values() {
        content.push(Block::ToolUse {
            id: call.id,
            name: call.name,
            input: input_of(&call.arguments),
            signature: None,
        });
    }
    Ok(Response {
        content,
        stop: openai_stop(finish.as_deref(), called, refused),
        usage,
        model,
        forgot_reasoning: false,
    })
}

impl OpenAiCompatible {
    fn url(&self) -> String {
        format!("{}/chat/completions", self.base.trim_end_matches('/'))
    }

    /// The header a request carries its key in: `Authorization: Bearer
    /// <key>`, or the key as it is in [`OpenAiCompatible::key_header`].
    fn key_headers(&self, key: Option<&str>) -> Vec<(String, String)> {
        match (key, &self.key_header) {
            (None, _) => Vec::new(),
            (Some(key), Some(header)) => vec![(header.clone(), key.to_string())],
            (Some(key), None) => vec![("Authorization".into(), format!("Bearer {key}"))],
        }
    }
}

impl OpenAiCompatible {
    /// Whether the server refused `max_tokens` for `max_completion_tokens`:
    /// from now on it is sent that, and `body` is changed to say it.
    fn takes_completion_tokens(&self, why: &str, body: &mut Value) -> bool {
        if !(why.contains("max_completion_tokens") && body.get("max_tokens").is_some()) {
            return false;
        }
        self.completion_tokens.store(true, Ordering::Relaxed);
        if let Some(fields) = body.as_object_mut() {
            if let Some(most) = fields.remove("max_tokens") {
                fields.insert("max_completion_tokens".into(), most);
            }
        }
        true
    }

    fn complete_once(&self, request: &Request) -> Result<Response, ModelError> {
        let mut body = self.body(request);
        let post = |body: &Value| {
            let bytes =
                serde_json::to_vec(body).map_err(|e| ModelError::Protocol(e.to_string()))?;
            self.keys.send(|key| {
                let owned = self.key_headers(key);
                let headers: Vec<(&str, &str)> = owned
                    .iter()
                    .map(|(n, v)| (n.as_str(), v.as_str()))
                    .collect();
                self.client
                    .post_json(&self.url(), &headers, &bytes, MAX_ANSWER)
                    .map_err(http_error)
            })
        };
        let mut answered = post(&body);
        // Each refusal it can be asked again without, once; how hard it
        // thinks, until a level it takes is found.
        for _ in 0..1 + Asked::TRIES {
            answered = match answered {
                Err(ModelError::Refused(why)) if self.takes_completion_tokens(&why, &mut body) => {
                    post(&body)
                }
                Err(ModelError::Refused(why)) if self.thinks_otherwise(&why, &mut body) => {
                    post(&body)
                }
                answered => answered,
            };
        }
        let answer = answered?;
        let value: Value = serde_json::from_slice(&answer.body)
            .map_err(|e| ModelError::Protocol(format!("answer is not JSON: {e}")))?;
        openai_response(&value)
    }

    /// Streams, asking for the usage in the last chunk; a server that
    /// refuses `stream_options` is asked again without it, one that
    /// refuses `max_tokens` with `max_completion_tokens`, and one that
    /// refuses how hard the model is to think at the nearest level it
    /// takes.
    fn stream_once(
        &self,
        request: &Request,
        seen: &mut dyn FnMut(Delta) -> bool,
    ) -> Result<Response, ModelError> {
        let mut body = self.body(request);
        body["stream"] = json!(true);
        body["stream_options"] = json!({"include_usage": true});
        let post = |body: &Value| {
            let bytes =
                serde_json::to_vec(body).map_err(|e| ModelError::Protocol(e.to_string()))?;
            self.keys.send(|key| {
                let owned = self.key_headers(key);
                let headers: Vec<(&str, &str)> = owned
                    .iter()
                    .map(|(n, v)| (n.as_str(), v.as_str()))
                    .collect();
                post_stream(&self.client, &self.url(), &headers, &bytes)
            })
        };
        let mut answered = post(&body);
        // Each refusal it can be asked again without, once; how hard it
        // thinks, until a level it takes is found.
        for _ in 0..2 + Asked::TRIES {
            answered = match answered {
                Err(ModelError::Refused(why))
                    if why.contains("stream_options") && body.get("stream_options").is_some() =>
                {
                    if let Some(fields) = body.as_object_mut() {
                        fields.remove("stream_options");
                    }
                    post(&body)
                }
                Err(ModelError::Refused(why)) if self.takes_completion_tokens(&why, &mut body) => {
                    post(&body)
                }
                Err(ModelError::Refused(why)) if self.thinks_otherwise(&why, &mut body) => {
                    post(&body)
                }
                answered => answered,
            };
        }
        let mut response = answered?;
        if !streams(&response) {
            return whole(response, openai_response, seen);
        }
        openai_stream(&mut Events::new(response.body(), MAX_EVENT), seen)
    }
}

impl Model for OpenAiCompatible {
    fn id(&self) -> &str {
        &self.model
    }

    fn complete(&self, request: &Request) -> Result<Response, ModelError> {
        kept_or_not(request, &self.source(), |request| {
            self.complete_once(request)
        })
    }

    fn stream(
        &self,
        request: &Request,
        seen: &mut dyn FnMut(Delta) -> bool,
    ) -> Result<Response, ModelError> {
        kept_or_not(request, &self.source(), |request| {
            self.stream_once(request, seen)
        })
    }

    fn most_tokens(&self, max_tokens: u32) -> u32 {
        most_tokens(self.thinking, max_tokens)
    }
}

/// What Gemini is sent for a call it did not make itself (one another
/// model made, earlier in the conversation), which has no thought
/// signature of Gemini's: the value its documentation gives for that, which
/// its check of signatures lets through.
pub const GEMINI_NO_SIGNATURE: &str = "skip_thought_signature_validator";

/// Google's Gemini API (`generateContent`, streamed as Server-Sent Events
/// with `streamGenerateContent?alt=sse`), with an API key (`x-goog-api-key`).
/// Its thinking is not told; the thought signature it gives with a call is
/// kept on the call and sent back with it, as its models since Gemini 3
/// require within a turn, and the one it gives with its text is kept
/// ([`Block::Reasoning`]) and sent back on the text, as it recommends. An
/// effort level is `thinkingLevel` (Gemini 3 and later: `minimal`, `low`,
/// `medium` or `high`, the nearest a model takes), a budget
/// `thinkingBudget` (Gemini 2.5); a model that refuses either is asked the
/// nearest it takes ([`Asked`]).
pub struct Gemini {
    client: sion_http::Client,
    /// The API's base URL, `https://generativelanguage.googleapis.com` by
    /// default.
    pub base: String,
    /// The model, for example `gemini-3-pro-preview`.
    pub model: String,
    keys: Keys,
    /// How hard it is asked to think, if said.
    pub thinking: Option<Thinking>,
    /// How hard it is asked to think, once it refused what is configured.
    asked: Asked,
}

/// How hard a Gemini model is asked to think, as Gemini names levels:
/// `none` is its least (`minimal`, or `low` for the models that have no
/// `minimal`), and anything above `high` is `high`.
pub(crate) fn gemini_thinking(model: &str, thinking: Option<Thinking>) -> Option<Thinking> {
    let name = crate::prices::normalized(model);
    // Gemini 4 Argon and Gemini 3.8 Flash refuse `minimal`.
    let least = if name.starts_with("gemini-4-argon") || name.starts_with("gemini-3.8-flash") {
        Effort::Low
    } else {
        Effort::Minimal
    };
    Some(match thinking? {
        Thinking::Effort(level) => Thinking::Effort(level.clamp(least, Effort::High)),
        budget => budget,
    })
}

/// Gemini's `thinkingConfig` for `thinking`, already as Gemini names
/// levels ([`gemini_thinking`]).
pub(crate) fn gemini_thinking_config(thinking: Thinking) -> Value {
    match thinking {
        Thinking::Effort(level) => json!({"thinkingLevel": match level {
            Effort::None | Effort::Minimal => "minimal",
            Effort::Low => "low",
            Effort::Medium => "medium",
            Effort::High | Effort::Xhigh | Effort::Max => "high",
        }}),
        Thinking::Budget(tokens) => json!({ "thinkingBudget": tokens }),
    }
}

impl std::fmt::Debug for Gemini {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Gemini({} at {})", self.model, self.base)
    }
}

impl Gemini {
    /// A provider for `model` with the API key `key` (several, separated
    /// by commas, are used in turn: [`Keys`]).
    pub fn new(client: sion_http::Client, model: &str, key: &str) -> Gemini {
        Gemini {
            client,
            base: "https://generativelanguage.googleapis.com".into(),
            model: model.into(),
            keys: Keys::parse(key),
            thinking: None,
            asked: Asked::default(),
        }
    }

    /// Where its reasoning comes from, and the only reasoning it is given
    /// back: its kind, base URL and model.
    pub fn source(&self) -> String {
        format!("gemini {} {}", self.base.trim_end_matches('/'), self.model)
    }

    /// How hard it is configured to think, as Gemini names levels.
    fn said(&self) -> Option<Thinking> {
        gemini_thinking(&self.model, self.thinking)
    }

    fn url(&self, stream: bool) -> String {
        let base = self.base.trim_end_matches('/');
        if stream {
            format!(
                "{base}/v1beta/models/{}:streamGenerateContent?alt=sse",
                self.model
            )
        } else {
            format!("{base}/v1beta/models/{}:generateContent", self.model)
        }
    }

    /// The request body for `request`: the system prompt as the system
    /// instruction, each message a content (`user` or `model`), the tools
    /// as function declarations with their JSON Schemas, and how hard it
    /// thinks, with room to.
    pub fn body(&self, request: &Request) -> Value {
        self.body_from(request, &self.source())
    }

    /// The request body for `request`, giving back the signatures kept from
    /// `from`: its own, or those of the platform it is reached through
    /// ([`crate::cloud::Vertex`]).
    pub(crate) fn body_from(&self, request: &Request, from: &str) -> Value {
        let mut body = gemini_body(request, from);
        body["generationConfig"]["maxOutputTokens"] = json!(self.most_tokens(request.max_tokens));
        if let Some(thinking) = self.asked.thinking(self.said()) {
            body["generationConfig"]["thinkingConfig"] = gemini_thinking_config(thinking);
        }
        body
    }
}

/// [`Gemini::body`], without its thinking: the signatures Gemini put on its
/// text are given back only for `from` ([`Gemini::source`]).
pub fn gemini_body(request: &Request, from: &str) -> Value {
    let mut pictures = Pictures::of(&request.messages, &request.images);
    // A function's response names its function: the calls' names by id.
    let names: BTreeMap<&str, &str> = request
        .messages
        .iter()
        .flat_map(|m| m.tool_uses())
        .map(|(id, name, _)| (id, name))
        .collect();
    let contents: Vec<Value> = request
        .messages
        .iter()
        .filter_map(|m| gemini_content(m, &names, &mut pictures, from))
        .collect();
    let mut body = json!({
        "contents": contents,
        "generationConfig": {"maxOutputTokens": request.max_tokens},
    });
    if !request.system.is_empty() {
        body["systemInstruction"] = json!({"parts": [{"text": request.system}]});
    }
    if !request.tools.is_empty() {
        let declarations: Vec<Value> = request
            .tools
            .iter()
            .map(|t| {
                json!({"name": t.name, "description": t.description,
                    "parametersJsonSchema": t.input_schema})
            })
            .collect();
        body["tools"] = json!([{ "functionDeclarations": declarations }]);
    }
    body
}

/// One message as a Gemini content, if it says anything.
fn gemini_content(
    message: &Message,
    names: &BTreeMap<&str, &str>,
    pictures: &mut Pictures<'_>,
    from: &str,
) -> Option<Value> {
    let role = match message.role {
        Role::User => "user",
        Role::Assistant => "model",
    };
    // The signature Gemini put on its text, given back on the text.
    let mut text_signature = message
        .content
        .iter()
        .filter_map(|b| kept_from(b, from))
        .find_map(|kept| kept["thoughtSignature"].as_str());
    // Calls another model made carry no signature of Gemini's: the first
    // of them is sent with the value that stands for one.
    let signed = message.content.iter().any(|b| {
        matches!(
            b,
            Block::ToolUse {
                signature: Some(_),
                ..
            }
        )
    });
    let mut stood_in = false;
    let mut parts = Vec::new();
    // The pictures results returned, after the last of the results: a
    // turn's function responses go together, nothing between them.
    let mut shown = Vec::new();
    for block in &message.content {
        match block {
            Block::Text { text } if !text.is_empty() => {
                let mut part = json!({ "text": text });
                if let Some(signature) = text_signature.take() {
                    part["thoughtSignature"] = json!(signature);
                }
                parts.push(part);
            }
            Block::Text { .. } | Block::Reasoning { .. } => {}
            Block::ToolUse {
                id,
                name,
                input,
                signature,
            } => {
                let mut part = json!({"functionCall": {"id": id, "name": name, "args": input}});
                match signature {
                    Some(signature) => part["thoughtSignature"] = json!(signature),
                    None if !signed && !stood_in => {
                        part["thoughtSignature"] = json!(GEMINI_NO_SIGNATURE);
                        stood_in = true;
                    }
                    None => {}
                }
                parts.push(part);
            }
            Block::ToolResult {
                id,
                content,
                is_error,
                images,
            } => {
                let name = names.get(id.as_str()).copied().unwrap_or("tool");
                let response = if *is_error {
                    json!({ "error": content })
                } else {
                    json!({ "output": content })
                };
                parts.push(
                    json!({"functionResponse": {"id": id, "name": name, "response": response}}),
                );
                for image in images {
                    shown.push(match pictures.next(image) {
                        Some(data) => {
                            json!({"inlineData": {"mimeType": image.media_type, "data": data}})
                        }
                        None => json!({ "text": IMAGE_NOT_SHOWN }),
                    });
                }
            }
            Block::Image { image } => parts.push(match pictures.next(image) {
                Some(data) => json!({"inlineData": {"mimeType": image.media_type, "data": data}}),
                None => json!({ "text": IMAGE_NOT_SHOWN }),
            }),
        }
    }
    // The responses first, then what else the message says, then the
    // pictures.
    let (mut responses, rest): (Vec<Value>, Vec<Value>) = parts
        .into_iter()
        .partition(|part| part.get("functionResponse").is_some());
    responses.extend(rest);
    responses.extend(shown);
    (!responses.is_empty()).then(|| json!({"role": role, "parts": responses}))
}

/// A call's id: Gemini's, or one made here, which names it uniquely in the
/// conversation.
fn gemini_call_id(call: &Value, at: usize) -> String {
    match call["id"].as_str().filter(|id| !id.is_empty()) {
        Some(id) => id.to_string(),
        None => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            format!("gemini_{now:x}_{at}")
        }
    }
}

fn gemini_usage(meta: &Value) -> Usage {
    let count = |name: &str| meta[name].as_u64().unwrap_or(0);
    Usage {
        // The prompt's tokens, those of the cached content among them.
        input: count("promptTokenCount"),
        // Its thinking is paid for as output.
        output: count("candidatesTokenCount") + count("thoughtsTokenCount"),
        cache_read: count("cachedContentTokenCount"),
        cache_write: 0,
    }
}

/// What Anthropic's `usage` counts: its `input_tokens` leave out the
/// tokens read from the cache and those written to it, which are input
/// all the same.
fn anthropic_usage(usage: &Value) -> Usage {
    let count = |name: &str| usage[name].as_u64().unwrap_or(0);
    let cache_read = count("cache_read_input_tokens");
    let cache_write = count("cache_creation_input_tokens");
    Usage {
        input: count("input_tokens") + cache_read + cache_write,
        output: count("output_tokens"),
        cache_read,
        cache_write,
    }
}

/// What a Chat Completions `usage` counts: the prompt's tokens, those read
/// from the cache and those written to it among them.
fn openai_usage(usage: &Value) -> Usage {
    let details = &usage["prompt_tokens_details"];
    Usage {
        input: usage["prompt_tokens"].as_u64().unwrap_or(0),
        output: usage["completion_tokens"].as_u64().unwrap_or(0),
        cache_read: details["cached_tokens"].as_u64().unwrap_or(0),
        cache_write: details["cache_write_tokens"].as_u64().unwrap_or(0),
    }
}

fn gemini_stop(reason: Option<&str>, called: bool) -> Stop {
    match reason {
        _ if called => Stop::ToolUse,
        Some("STOP") | None => Stop::EndTurn,
        Some("MAX_TOKENS") => Stop::MaxTokens,
        Some(
            "SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII" | "IMAGE_SAFETY",
        ) => Stop::Refusal,
        Some(other) => Stop::Other(other.to_string()),
    }
}

/// An error Gemini reports in its answer (`{"error": {"code", "message",
/// "status"}}`), as the status it stands for.
fn gemini_error(error: &Value) -> ModelError {
    let status = error["status"].as_str().unwrap_or("UNKNOWN");
    let said = format!(
        "{status}: {}",
        error["message"].as_str().unwrap_or("no message")
    );
    match status {
        "RESOURCE_EXHAUSTED" | "UNAVAILABLE" | "INTERNAL" | "DEADLINE_EXCEEDED" => {
            ModelError::Busy(said)
        }
        _ => ModelError::Refused(said),
    }
}

/// The blocks of an answer of Gemini's: its text, the signature it put on
/// its text, kept, then its calls.
fn gemini_blocks(text: String, signed: Option<String>, calls: Vec<Block>) -> Vec<Block> {
    let mut content = Vec::new();
    if !text.is_empty() {
        content.push(Block::text(text));
    }
    if let Some(signature) = signed {
        content.push(Block::Reasoning {
            from: String::new(),
            data: json!({ "thoughtSignature": signature }),
        });
    }
    content.extend(calls);
    content
}

/// Reads a whole `generateContent` answer: the text of its first
/// candidate, a block, the signature on its text, then its calls, each
/// with its signature; its thinking left out.
pub fn gemini_response(answer: &Value) -> Result<Response, ModelError> {
    if answer["error"].is_object() {
        return Err(gemini_error(&answer["error"]));
    }
    let model = answer["modelVersion"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let usage = gemini_usage(&answer["usageMetadata"]);
    // A prompt Gemini blocked has no candidate.
    if answer["promptFeedback"]["blockReason"].is_string() {
        return Ok(Response {
            content: Vec::new(),
            stop: Stop::Refusal,
            usage,
            model,
            forgot_reasoning: false,
        });
    }
    let candidate = &answer["candidates"][0];
    if !candidate.is_object() {
        return Err(ModelError::Protocol("the answer has no candidate".into()));
    }
    let mut text = String::new();
    let mut signed = None;
    let mut calls = Vec::new();
    for part in candidate["content"]["parts"]
        .as_array()
        .into_iter()
        .flatten()
    {
        gemini_part(part, &mut text, &mut signed, &mut calls);
    }
    let stop = gemini_stop(candidate["finishReason"].as_str(), !calls.is_empty());
    Ok(Response {
        content: gemini_blocks(text, signed, calls),
        stop,
        usage,
        model,
        forgot_reasoning: false,
    })
}

/// Takes one part of an answer: its text onto `text`, the first signature
/// on a part that is not a call into `signed` (a streamed answer may give
/// it on a part with no text), a call onto `calls`; thinking left out. What
/// it adds, for a stream to tell.
fn gemini_part(
    part: &Value,
    text: &mut String,
    signed: &mut Option<String>,
    calls: &mut Vec<Block>,
) -> Option<Delta> {
    if part["thought"] == true {
        return None;
    }
    if part.get("functionCall").is_none() {
        if let Some(signature) = part["thoughtSignature"].as_str().filter(|s| !s.is_empty()) {
            signed.get_or_insert_with(|| signature.to_string());
        }
    }
    if let Some(more) = part["text"].as_str().filter(|t| !t.is_empty()) {
        text.push_str(more);
        return Some(Delta::Text(more.to_string()));
    }
    let call = part.get("functionCall")?;
    let name = call["name"].as_str().unwrap_or_default().to_string();
    calls.push(Block::ToolUse {
        id: gemini_call_id(call, calls.len()),
        name: name.clone(),
        input: if call["args"].is_object() {
            call["args"].clone()
        } else {
            json!({})
        },
        signature: part["thoughtSignature"].as_str().map(str::to_string),
    });
    Some(Delta::Call(name))
}

/// Reads a streamed answer (`streamGenerateContent?alt=sse`) from its
/// events, telling `seen` its text and calls as they come: each event is a
/// part of the answer, and the answer is what [`gemini_response`] reads
/// from them all. It is whole once a candidate says why it finished.
pub fn gemini_stream<R: Read>(
    events: &mut Events<R>,
    seen: &mut dyn FnMut(Delta) -> bool,
) -> Result<Response, ModelError> {
    let mut text = String::new();
    let mut signed = None;
    let mut calls = Vec::new();
    let mut finish: Option<String> = None;
    let mut usage = Usage::default();
    let mut model = String::new();
    let mut cut = false;
    while let Some(event) = events.next_event() {
        let event = event.map_err(ModelError::Transport)?;
        let Ok(chunk) = serde_json::from_str::<Value>(&event.data) else {
            continue;
        };
        if chunk["error"].is_object() {
            return Err(gemini_error(&chunk["error"]));
        }
        if model.is_empty() {
            model = chunk["modelVersion"]
                .as_str()
                .unwrap_or_default()
                .to_string();
        }
        if chunk["usageMetadata"].is_object() {
            usage = gemini_usage(&chunk["usageMetadata"]);
        }
        if chunk["promptFeedback"]["blockReason"].is_string() {
            return Ok(Response {
                content: Vec::new(),
                stop: Stop::Refusal,
                usage,
                model,
                forgot_reasoning: false,
            });
        }
        let candidate = &chunk["candidates"][0];
        for part in candidate["content"]["parts"]
            .as_array()
            .into_iter()
            .flatten()
        {
            if let Some(delta) = gemini_part(part, &mut text, &mut signed, &mut calls) {
                cut = cut || !seen(delta);
            }
        }
        if let Some(reason) = candidate["finishReason"].as_str() {
            finish = Some(reason.to_string());
        }
        // Cut off: nothing more is read, and the connection closes.
        if cut {
            break;
        }
    }
    if cut {
        return Ok(cut_off([text], usage, model));
    }
    if finish.is_none() {
        return Err(ModelError::Transport(
            "the answer stopped before its end".into(),
        ));
    }
    let stop = gemini_stop(finish.as_deref(), !calls.is_empty());
    Ok(Response {
        content: gemini_blocks(text, signed, calls),
        stop,
        usage,
        model,
        forgot_reasoning: false,
    })
}

impl Model for Gemini {
    fn id(&self) -> &str {
        &self.model
    }

    fn complete(&self, request: &Request) -> Result<Response, ModelError> {
        signed_or_not(request, &self.source(), |request| {
            self.complete_once(request)
        })
    }

    fn stream(
        &self,
        request: &Request,
        seen: &mut dyn FnMut(Delta) -> bool,
    ) -> Result<Response, ModelError> {
        signed_or_not(request, &self.source(), |request| {
            self.stream_once(request, seen)
        })
    }

    fn most_tokens(&self, max_tokens: u32) -> u32 {
        most_tokens(self.thinking, max_tokens)
    }
}

impl Gemini {
    fn complete_once(&self, request: &Request) -> Result<Response, ModelError> {
        let answer = thought_as_taken(&self.asked, self.said(), || {
            let body = serde_json::to_vec(&self.body(request))
                .map_err(|e| ModelError::Protocol(e.to_string()))?;
            self.keys.send(|key| {
                self.client
                    .post_json(
                        &self.url(false),
                        &[("x-goog-api-key", key.unwrap_or_default())],
                        &body,
                        MAX_ANSWER,
                    )
                    .map_err(http_error)
            })
        })?;
        let value: Value = serde_json::from_slice(&answer.body)
            .map_err(|e| ModelError::Protocol(format!("answer is not JSON: {e}")))?;
        gemini_response(&value)
    }

    fn stream_once(
        &self,
        request: &Request,
        seen: &mut dyn FnMut(Delta) -> bool,
    ) -> Result<Response, ModelError> {
        let mut response = thought_as_taken(&self.asked, self.said(), || {
            let body = serde_json::to_vec(&self.body(request))
                .map_err(|e| ModelError::Protocol(e.to_string()))?;
            self.keys.send(|key| {
                post_stream(
                    &self.client,
                    &self.url(true),
                    &[("x-goog-api-key", key.unwrap_or_default())],
                    &body,
                )
            })
        })?;
        if !streams(&response) {
            return whole(response, gemini_response, seen);
        }
        gemini_stream(&mut Events::new(response.body(), MAX_EVENT), seen)
    }
}

/// Where a provider's bearer comes from ([`OpenAiResponses`]).
pub enum Bearer {
    /// API keys, used in turn ([`Keys`]).
    Keys(Keys),
    /// A token a sign-in gave: `token(false)` the one to use, `token(true)`
    /// a new one, after the provider refused the last.
    Token(Box<dyn Fn(bool) -> Result<String, ModelError> + Send + Sync>),
}

impl std::fmt::Debug for Bearer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Bearer::Keys(keys) => write!(f, "Bearer::{keys:?}"),
            Bearer::Token(_) => write!(f, "Bearer::Token"),
        }
    }
}

/// Whether a model of the Responses API returns its reasoning, encrypted:
/// not known yet.
const REASONING_UNKNOWN: u8 = 0;
/// It does.
const REASONING_RETURNED: u8 = 1;
/// It does not: asked for it, it refuses the request.
const REASONING_REFUSED: u8 = 2;

/// OpenAI's Responses API (`POST {base}/responses`), streamed and not
/// stored (`"store": false`): each call carries the whole conversation, as
/// the chat completions do. Its reasoning is asked for, encrypted
/// (`include: ["reasoning.encrypted_content"]`), kept, and given back as
/// items before what followed them, without their ids, since nothing is
/// stored; a model that refuses to give it is not asked again. An effort
/// level is `reasoning.effort`, the nearest the model takes once it
/// refuses the one configured ([`Asked`]).
pub struct OpenAiResponses {
    client: sion_http::Client,
    /// The API's base URL, `https://api.openai.com/v1` by default.
    pub base: String,
    /// The model.
    pub model: String,
    bearer: Bearer,
    /// How hard it is asked to think, if said.
    pub thinking: Option<Thinking>,
    /// Whether it returns its reasoning ([`REASONING_UNKNOWN`] and the
    /// rest).
    reasoning: AtomicU8,
    /// How hard it is asked to think, once it refused what is configured.
    asked: Asked,
}

impl std::fmt::Debug for OpenAiResponses {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "OpenAiResponses({} at {})", self.model, self.base)
    }
}

impl OpenAiResponses {
    /// A provider for `model`, its bearer from `bearer`.
    pub fn new(client: sion_http::Client, model: &str, bearer: Bearer) -> OpenAiResponses {
        OpenAiResponses {
            client,
            base: "https://api.openai.com/v1".into(),
            model: model.into(),
            bearer,
            thinking: None,
            reasoning: AtomicU8::new(REASONING_UNKNOWN),
            asked: Asked::default(),
        }
    }

    /// Where its reasoning comes from, and the only reasoning it is given
    /// back: its kind, base URL and model.
    pub fn source(&self) -> String {
        format!(
            "openai-responses {} {}",
            self.base.trim_end_matches('/'),
            self.model
        )
    }

    /// The request body for `request` ([`responses_body`]), with how hard
    /// it thinks and room to, and asking for its reasoning unless it
    /// refused to give it.
    pub fn body(&self, request: &Request) -> Value {
        let mut body = responses_body(&self.model, request, &self.source());
        body["max_output_tokens"] = json!(self.most_tokens(request.max_tokens));
        if self.reasoning.load(Ordering::Relaxed) != REASONING_REFUSED {
            body["include"] = json!(["reasoning.encrypted_content"]);
        }
        if let Some(Thinking::Effort(level)) = self.asked.thinking(self.said()) {
            body["reasoning"] = json!({ "effort": openai_effort(level) });
        }
        body
    }

    /// How hard it is configured to think, as its model takes levels.
    fn said(&self) -> Option<Thinking> {
        openai_thinking(&self.model, self.thinking)
    }

    fn url(&self) -> String {
        format!("{}/responses", self.base.trim_end_matches('/'))
    }

    /// Posts `body` with the bearer: keys in turn, or the token, a new one
    /// asked for once if the provider refuses it.
    fn post(&self, body: &[u8]) -> Result<sion_http::Response, ModelError> {
        let send = |token: &str| {
            let bearer = format!("Bearer {token}");
            post_stream(
                &self.client,
                &self.url(),
                &[("Authorization", bearer.as_str())],
                body,
            )
        };
        match &self.bearer {
            Bearer::Keys(keys) => keys.send(|key| send(key.unwrap_or_default())),
            Bearer::Token(token) => match send(&token(false)?) {
                Err(ModelError::Refused(why)) if why.starts_with("HTTP 401") => send(&token(true)?),
                answered => answered,
            },
        }
    }
}

/// The body of a Responses API request for `request`: the system prompt as
/// the instructions, each message an input item (the model's calls and
/// their outputs items of their own, the pictures results returned in a
/// user message after them), the reasoning kept from `from` an item before
/// what followed it, the tools as functions; streamed, not stored.
pub fn responses_body(model: &str, request: &Request, from: &str) -> Value {
    let mut pictures = Pictures::of(&request.messages, &request.images);
    let mut input = Vec::new();
    for message in &request.messages {
        match message.role {
            Role::User => {
                let mut parts = Vec::new();
                let mut shown = Vec::new();
                for block in &message.content {
                    match block {
                        Block::Text { text } if !text.is_empty() => {
                            parts.push(json!({"type": "input_text", "text": text}));
                        }
                        Block::ToolResult {
                            id,
                            content,
                            is_error,
                            images,
                        } => {
                            let output = if *is_error {
                                format!("error: {content}")
                            } else {
                                content.clone()
                            };
                            input.push(json!({"type": "function_call_output", "call_id": id,
                                "output": output}));
                            for image in images {
                                shown.push(match pictures.next(image) {
                                    Some(data) => json!({"type": "input_image",
                                        "image_url": format!("data:{};base64,{data}", image.media_type)}),
                                    None => json!({"type": "input_text", "text": IMAGE_NOT_SHOWN}),
                                });
                            }
                        }
                        Block::Image { image } => parts.push(match pictures.next(image) {
                            Some(data) => json!({"type": "input_image",
                                "image_url": format!("data:{};base64,{data}", image.media_type)}),
                            None => json!({"type": "input_text", "text": IMAGE_NOT_SHOWN}),
                        }),
                        _ => {}
                    }
                }
                parts.extend(shown);
                if !parts.is_empty() {
                    input.push(json!({"role": "user", "content": parts}));
                }
            }
            Role::Assistant => {
                for (at, block) in message.content.iter().enumerate() {
                    match block {
                        Block::Text { text } if !text.is_empty() => input
                            .push(json!({"type": "message", "role": "assistant",
                                "content": [{"type": "output_text", "text": text}]})),
                        Block::ToolUse {
                            id,
                            name,
                            input: arguments,
                            ..
                        } => input.push(json!({"type": "function_call", "call_id": id,
                            "name": name, "arguments": arguments.to_string()})),
                        // A reasoning item goes before what followed it,
                        // and only with something following it.
                        Block::Reasoning { .. } => {
                            let followed = message.content[at + 1..].iter().any(|b| {
                                matches!(b, Block::ToolUse { .. })
                                    || matches!(b, Block::Text { text } if !text.is_empty())
                            });
                            if let Some(kept) = kept_from(block, from).filter(|_| followed) {
                                input.push(json!({"type": "reasoning",
                                    "summary": kept.get("summary").cloned().unwrap_or(json!([])),
                                    "encrypted_content": kept["encrypted_content"]}));
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    let mut body = json!({
        "model": model,
        "input": input,
        "max_output_tokens": request.max_tokens,
        "store": false,
        "stream": true,
    });
    if !request.system.is_empty() {
        body["instructions"] = json!(request.system);
    }
    if !request.tools.is_empty() {
        let tools: Vec<Value> = request
            .tools
            .iter()
            .map(|t| {
                json!({"type": "function", "name": t.name, "description": t.description,
                    "parameters": t.input_schema, "strict": false})
            })
            .collect();
        body["tools"] = json!(tools);
    }
    body
}

/// Reads a whole Responses API answer (a `response` object): its items in
/// order, its reasoning kept where it returned it encrypted, its text one
/// block where its first message was, and its calls.
pub fn responses_response(answer: &Value) -> Result<Response, ModelError> {
    if answer["status"] == "failed" || answer["error"].is_object() {
        return Err(responses_error(&answer["error"]));
    }
    let mut content = Vec::new();
    let mut text = String::new();
    // Where the text goes: where its first message was.
    let mut text_at = None;
    let mut called = false;
    let mut refused = false;
    for item in answer["output"].as_array().into_iter().flatten() {
        match item["type"].as_str() {
            Some("message") => {
                text_at.get_or_insert(content.len());
                for part in item["content"].as_array().into_iter().flatten() {
                    match part["type"].as_str() {
                        Some("output_text") => text.push_str(part["text"].as_str().unwrap_or("")),
                        Some("refusal") => {
                            refused = true;
                            text.push_str(part["refusal"].as_str().unwrap_or(""));
                        }
                        _ => {}
                    }
                }
            }
            Some("function_call") => {
                called = true;
                content.push(Block::ToolUse {
                    id: item["call_id"].as_str().unwrap_or_default().to_string(),
                    name: item["name"].as_str().unwrap_or_default().to_string(),
                    input: input_of(item["arguments"].as_str().unwrap_or_default()),
                    signature: None,
                });
            }
            Some("reasoning") => {
                if let Some(encrypted) =
                    item["encrypted_content"].as_str().filter(|e| !e.is_empty())
                {
                    content.push(Block::Reasoning {
                        from: String::new(),
                        data: json!({
                            "summary": item.get("summary").cloned().unwrap_or(json!([])),
                            "encrypted_content": encrypted,
                        }),
                    });
                }
            }
            _ => {}
        }
    }
    if !text.is_empty() {
        content.insert(text_at.unwrap_or(0), Block::text(text));
    }
    // Cut off (`incomplete`) whatever it had begun, calls included: a
    // call it did not finish is not one to make.
    let incomplete = (answer["status"] == "incomplete").then(|| {
        answer["incomplete_details"]["reason"]
            .as_str()
            .unwrap_or("incomplete")
    });
    let stop = match incomplete {
        Some("max_output_tokens") => Stop::MaxTokens,
        Some("content_filter") => Stop::Refusal,
        _ if refused => Stop::Refusal,
        _ if called => Stop::ToolUse,
        Some(other) => Stop::Other(other.to_string()),
        None => Stop::EndTurn,
    };
    Ok(Response {
        content,
        stop,
        usage: Usage {
            // Its input's tokens, those read from the cache and those
            // written to it among them.
            input: answer["usage"]["input_tokens"].as_u64().unwrap_or(0),
            output: answer["usage"]["output_tokens"].as_u64().unwrap_or(0),
            cache_read: answer["usage"]["input_tokens_details"]["cached_tokens"]
                .as_u64()
                .unwrap_or(0),
            cache_write: answer["usage"]["input_tokens_details"]["cache_write_tokens"]
                .as_u64()
                .unwrap_or(0),
        },
        model: answer["model"].as_str().unwrap_or_default().to_string(),
        forgot_reasoning: false,
    })
}

/// An error a Responses API answer reports: a limit or a failure on the
/// provider's side may pass (another model may answer meanwhile); the rest
/// does not.
fn responses_error(error: &Value) -> ModelError {
    let code = error["code"]
        .as_str()
        .or_else(|| error["type"].as_str())
        .unwrap_or("error");
    let said = format!(
        "{code}: {}",
        error["message"].as_str().unwrap_or("no message")
    );
    if spent(&said) {
        // Credit or quota spent is not over in a minute.
        ModelError::Limited {
            message: said,
            retry_after: REFUSED_REST.as_secs(),
        }
    } else if code.contains("rate_limit")
        || code.contains("server_error")
        || code.contains("usage_limit")
        || code.contains("usage_unavailable")
        || code.contains("overloaded")
        || code.contains("unavailable")
        || code.contains("timeout")
    {
        match asked_wait(&said) {
            Some(retry_after) => ModelError::Limited {
                message: said,
                retry_after,
            },
            None => ModelError::Busy(said),
        }
    } else if too_long(&said) {
        ModelError::TooLong(said)
    } else {
        ModelError::Refused(said)
    }
}

/// Reads a streamed Responses API answer from its events, telling `seen`
/// the text as it comes and each call as it begins: the answer is the one
/// the terminal event carries (`response.completed`, or
/// `response.incomplete`), as [`responses_response`] reads it, its items
/// those the stream gave (`response.output_item.done`) if it lists none.
/// One that ends without a terminal event is a transport failure.
pub fn responses_stream<R: Read>(
    events: &mut Events<R>,
    seen: &mut dyn FnMut(Delta) -> bool,
) -> Result<Response, ModelError> {
    let mut text = String::new();
    let mut cut = false;
    let mut model = String::new();
    let mut items = Vec::new();
    while let Some(event) = events.next_event() {
        let event = event.map_err(ModelError::Transport)?;
        let Ok(data) = serde_json::from_str::<Value>(&event.data) else {
            continue;
        };
        let kind = data["type"]
            .as_str()
            .unwrap_or(event.name.as_str())
            .to_string();
        match kind.as_str() {
            "response.created" | "response.in_progress" => {
                if model.is_empty() {
                    model = data["response"]["model"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                }
            }
            "response.output_text.delta" => {
                let more = data["delta"].as_str().unwrap_or_default();
                if !more.is_empty() {
                    text.push_str(more);
                    cut = !seen(Delta::Text(more.to_string()));
                }
            }
            "response.output_item.added" if data["item"]["type"] == "function_call" => {
                let name = data["item"]["name"].as_str().unwrap_or_default();
                cut = !seen(Delta::Call(name.to_string()));
            }
            "response.output_item.done" if data["item"].is_object() => {
                items.push(data["item"].clone());
            }
            "response.completed" | "response.incomplete" => {
                let mut answer = data["response"].clone();
                let listed = answer["output"].as_array().is_some_and(|o| !o.is_empty());
                if !listed && !items.is_empty() && answer.is_object() {
                    answer["output"] = Value::Array(items);
                }
                return responses_response(&answer);
            }
            "response.failed" => return Err(responses_error(&data["response"]["error"])),
            "error" => return Err(responses_error(&data)),
            _ => {}
        }
        // Cut off: nothing more is read, and the connection closes.
        if cut {
            return Ok(cut_off([text], Usage::default(), model));
        }
    }
    Err(ModelError::Transport(
        "the answer stopped before its end".into(),
    ))
}

impl Model for OpenAiResponses {
    fn id(&self) -> &str {
        &self.model
    }

    /// Streamed, whatever is asked: a sign-in's token is good for streamed
    /// requests only.
    fn complete(&self, request: &Request) -> Result<Response, ModelError> {
        self.stream(request, &mut |_| true)
    }

    /// A request it refuses for how hard the model is asked to think is
    /// sent again at the nearest level the model takes. One it refuses as
    /// malformed while it asks for the model's reasoning, or carries it,
    /// is sent once more without either; if that one goes through and none
    /// had before, the model is taken not to return its reasoning, and not
    /// asked for it again, and the reasoning the conversation kept is let
    /// go.
    fn stream(
        &self,
        request: &Request,
        seen: &mut dyn FnMut(Delta) -> bool,
    ) -> Result<Response, ModelError> {
        let from = self.source();
        let asked = self.reasoning.load(Ordering::Relaxed) != REASONING_REFUSED;
        let kept = keeps_reasoning(&request.messages, &from);
        let send = |request: &Request, bare: bool| {
            thought_as_taken(&self.asked, self.said(), || {
                let mut body = self.body(request);
                if bare {
                    if let Some(fields) = body.as_object_mut() {
                        fields.remove("include");
                    }
                }
                let bytes =
                    serde_json::to_vec(&body).map_err(|e| ModelError::Protocol(e.to_string()))?;
                self.post(&bytes)
            })
        };
        let mut forgot = false;
        let mut response = match send(request, false) {
            Err(ModelError::Refused(why)) if why.starts_with("HTTP 400") && (asked || kept) => {
                let without = Request {
                    messages: without_reasoning(&request.messages),
                    ..request.clone()
                };
                let response = send(&without, true)?;
                if asked {
                    let _ = self.reasoning.compare_exchange(
                        REASONING_UNKNOWN,
                        REASONING_REFUSED,
                        Ordering::Relaxed,
                        Ordering::Relaxed,
                    );
                }
                forgot = kept;
                response
            }
            answered => {
                let response = answered?;
                if asked {
                    let _ = self.reasoning.compare_exchange(
                        REASONING_UNKNOWN,
                        REASONING_RETURNED,
                        Ordering::Relaxed,
                        Ordering::Relaxed,
                    );
                }
                response
            }
        };
        let answered = if !streams(&response) {
            whole(response, responses_response, seen)
        } else {
            responses_stream(&mut Events::new(response.body(), MAX_EVENT), seen)
        };
        answered.map(|response| {
            let response = reasoning_from(response, &from);
            Response {
                forgot_reasoning: response.forgot_reasoning || forgot,
                ..response
            }
        })
    }

    fn most_tokens(&self, max_tokens: u32) -> u32 {
        most_tokens(self.thinking, max_tokens)
    }
}
