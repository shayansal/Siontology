//! What MCP servers ask of this client while it waits on them: a person's
//! input (elicitation) and a completion by the client's model (sampling).
//!
//! A modern server (2026-07-28) asks by answering a request with an
//! `input_required` result whose `inputRequests` hold what it needs; the
//! client gathers the answers and makes the request again with them
//! (`inputResponses`) and the server's `requestState` as it came
//! ([`crate::mcp`]). An earlier server sends its own requests
//! (`elicitation/create`, `sampling/createMessage`) while a request of the
//! client's is open. Either way the [`Responder`] answers, and the client
//! says what it can answer in its capabilities: elicitation only when a
//! person can be asked (an [`Asker`]), sampling only when the operator let
//! that server use a model, within limits ([`Sampler`]).
//!
//! **Elicitation.** A form is a flat object of text, numbers, yes-or-no
//! and choices; the person is asked in words, in the chat or terminal the
//! session answers, the server named and its message quoted, and answers
//! with `name: value` lines (or the value alone, for one field), checked
//! against the form before they are sent; or declines, or cancels. A form
//! that asks for a password, a key, a token or a card is declined without
//! asking: servers must not ask for those in a form. An address the server
//! wants the person to open is shown whole, its host apart, and never
//! opened or fetched by the client: the person opens it, and says when.
//!
//! **Sampling** is deprecated by the 2026-07-28 specification and still in
//! it; this client answers it for the servers the operator allows, without
//! the session's own conversation (`includeContext` is taken as `none`).

use std::sync::{Arc, Mutex};

use serde_json::{json, Map, Value};

use crate::model::Usage;

/// The longest message of a server's shown to a person, in characters.
pub const MAX_ASKED: usize = 1_000;

/// The most fields a form may have.
pub const MAX_FIELDS: usize = 20;

/// Asks a person, in words, and hears their reply.
pub trait Asker: Send + Sync {
    /// Asks `question`; the person's reply, or `None` if none came in time.
    fn ask(&self, question: &str) -> Option<String>;
    /// Tells the person something that needs no reply.
    fn tell(&self, text: &str) {
        let _ = text;
    }
}

/// Writes for a server with the client's model.
pub trait Sampler: Send + Sync {
    /// The `CreateMessageResult` for `params` (a `sampling/createMessage`
    /// request's), and the tokens it used.
    fn sample(&self, server: &str, params: &Value) -> Result<(Value, Usage), String>;
}

/// What answers a server's requests of this client.
#[derive(Clone, Default)]
pub struct Responder {
    /// Who answers elicitations, if anyone does.
    pub asker: Option<Arc<dyn Asker>>,
    /// What answers sampling, if the server may sample.
    pub sampler: Option<Arc<dyn Sampler>>,
    /// The tokens sampling used, until taken ([`Responder::take_spent`]).
    pub spent: Arc<Mutex<Usage>>,
}

impl std::fmt::Debug for Responder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Responder")
            .field("asker", &self.asker.is_some())
            .field("sampler", &self.sampler.is_some())
            .finish()
    }
}

impl Responder {
    /// The client capabilities this responder can stand behind.
    pub fn capabilities(&self) -> Value {
        let mut capabilities = json!({});
        if self.asker.is_some() {
            capabilities["elicitation"] = json!({"form": {}, "url": {}});
        }
        if self.sampler.is_some() {
            capabilities["sampling"] = json!({});
        }
        capabilities
    }

    /// The result for the request `method` with `params` that the server
    /// `server` makes, or why there is none (a JSON-RPC error's code and
    /// message).
    pub fn answer(
        &self,
        server: &str,
        method: &str,
        params: &Value,
    ) -> Result<Value, (i64, String)> {
        match method {
            "ping" => Ok(json!({})),
            "roots/list" => Ok(json!({"roots": []})),
            "elicitation/create" => match &self.asker {
                Some(asker) => Ok(elicit(asker.as_ref(), server, params)),
                None => Err((-32601, "no person can be asked here".into())),
            },
            "sampling/createMessage" => match &self.sampler {
                Some(sampler) => {
                    let (result, used) = sampler.sample(server, params).map_err(|e| (-1, e))?;
                    let mut spent = self.spent.lock().unwrap_or_else(|p| p.into_inner());
                    spent.input += used.input;
                    spent.output += used.output;
                    Ok(result)
                }
                None => Err((-32601, "this server may not sample".into())),
            },
            _ => Err((-32601, "this client does not offer that".into())),
        }
    }

    /// The JSON-RPC answer to `message`, a request from the server
    /// `server`.
    pub fn respond(&self, server: &str, message: &Value) -> Value {
        let id = message["id"].clone();
        let method = message["method"].as_str().unwrap_or_default();
        match self.answer(server, method, &message["params"]) {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err((code, why)) => {
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": why}})
            }
        }
    }

    /// The tokens sampling used since this was last asked.
    pub fn take_spent(&self) -> Option<Usage> {
        let mut spent = self.spent.lock().unwrap_or_else(|p| p.into_inner());
        if spent.input == 0 && spent.output == 0 {
            return None;
        }
        Some(std::mem::take(&mut *spent))
    }
}

/// A field of a form.
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    /// The name its value goes under.
    pub name: String,
    /// What it is called, for the person.
    pub title: Option<String>,
    /// What it is for.
    pub description: Option<String>,
    /// What it takes.
    pub kind: Kind,
    /// Whether it must be given.
    pub required: bool,
    /// What it holds unless the person says otherwise.
    pub default: Option<Value>,
}

/// What a field takes.
#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    /// Text, of `format` (`email`, `uri`, `date`, `date-time`) if given,
    /// between `min` and `max` characters.
    Text {
        /// Its format.
        format: Option<String>,
        /// The fewest characters.
        min: Option<u64>,
        /// The most characters.
        max: Option<u64>,
    },
    /// A number (a whole one if `integer`), between `min` and `max`.
    Number {
        /// Whether it is whole.
        integer: bool,
        /// The least.
        min: Option<f64>,
        /// The most.
        max: Option<f64>,
    },
    /// Yes or no.
    Boolean,
    /// One of `options` (each a value and what it is called), or several
    /// if `multiple`, between `min` and `max` of them.
    Choice {
        /// The values and their titles.
        options: Vec<(String, String)>,
        /// Whether several may be chosen.
        multiple: bool,
        /// The fewest chosen.
        min: Option<u64>,
        /// The most chosen.
        max: Option<u64>,
    },
}

/// What a server asks a person for.
#[derive(Clone, Debug, PartialEq)]
pub enum Elicitation {
    /// To fill in a form.
    Form {
        /// Why.
        message: String,
        /// Its fields.
        fields: Vec<Field>,
    },
    /// To open an address, outside the client.
    Url {
        /// Why.
        message: String,
        /// The address.
        url: String,
    },
}

/// Words that mark a field asking for a secret, which a form must not.
const SECRET_WORDS: [&str; 12] = [
    "password",
    "passcode",
    "passphrase",
    "secret",
    "api key",
    "apikey",
    "api_key",
    "token",
    "card number",
    "cvv",
    "cvc",
    "private key",
];

fn cut(text: &str, most: usize) -> String {
    if text.chars().count() <= most {
        text.to_string()
    } else {
        let mut out: String = text.chars().take(most).collect();
        out.push('…');
        out
    }
}

impl Elicitation {
    /// The request in `params` (an `elicitation/create` request's).
    pub fn parse(params: &Value) -> Result<Elicitation, String> {
        let message = cut(
            params["message"].as_str().unwrap_or_default().trim(),
            MAX_ASKED,
        );
        match params["mode"].as_str().unwrap_or("form") {
            "url" => {
                let url = params["url"]
                    .as_str()
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                if !(url.starts_with("https://") || url.starts_with("http://")) || url.len() > 2048
                {
                    return Err("the server gave no address to open".into());
                }
                Ok(Elicitation::Url { message, url })
            }
            "form" => {
                let schema = &params["requestedSchema"];
                let properties = schema["properties"]
                    .as_object()
                    .cloned()
                    .unwrap_or_default();
                if properties.len() > MAX_FIELDS {
                    return Err(format!("the form has more than {MAX_FIELDS} fields"));
                }
                let required: Vec<&str> = schema["required"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect();
                let mut fields = Vec::new();
                for (name, property) in &properties {
                    fields.push(field(name, property, required.contains(&name.as_str()))?);
                }
                Ok(Elicitation::Form { message, fields })
            }
            other => Err(format!(
                "the server asked in a way this client does not know ({other})"
            )),
        }
    }

    /// Whether a form asks for a secret, which servers must not.
    pub fn asks_for_a_secret(&self) -> bool {
        let Elicitation::Form { fields, message } = self else {
            return false;
        };
        // A form of one field is about what its message says.
        let said = if fields.len() == 1 {
            message.as_str()
        } else {
            ""
        };
        fields.iter().any(|f| {
            let words = format!(
                "{} {} {} {said}",
                f.name.replace(['_', '-'], " "),
                f.title.clone().unwrap_or_default(),
                f.description.clone().unwrap_or_default()
            )
            .to_lowercase();
            SECRET_WORDS.iter().any(|w| words.contains(w))
        })
    }
}

fn options_of(list: &Value) -> Vec<(String, String)> {
    list.as_array()
        .into_iter()
        .flatten()
        .filter_map(|o| {
            let value = o["const"].as_str()?.to_string();
            let title = o["title"].as_str().unwrap_or(&value).to_string();
            Some((value, title))
        })
        .collect()
}

fn field(name: &str, property: &Value, required: bool) -> Result<Field, String> {
    let text = |key: &str| property[key].as_str().map(|s| cut(s, 200));
    let kind = match property["type"].as_str() {
        Some("string") if property.get("enum").is_some() || property.get("oneOf").is_some() => {
            let options = match property.get("enum") {
                Some(values) => values
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(|v| (v.to_string(), v.to_string()))
                    .collect(),
                None => options_of(&property["oneOf"]),
            };
            Kind::Choice {
                options,
                multiple: false,
                min: None,
                max: None,
            }
        }
        Some("string") => Kind::Text {
            format: text("format"),
            min: property["minLength"].as_u64(),
            max: property["maxLength"].as_u64(),
        },
        Some(kind @ ("number" | "integer")) => Kind::Number {
            integer: kind == "integer",
            min: property["minimum"].as_f64(),
            max: property["maximum"].as_f64(),
        },
        Some("boolean") => Kind::Boolean,
        Some("array") => {
            let items = &property["items"];
            let options: Vec<(String, String)> = match items.get("enum") {
                Some(values) => values
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(|v| (v.to_string(), v.to_string()))
                    .collect(),
                None => options_of(&items["anyOf"]),
            };
            if options.is_empty() {
                return Err(format!(
                    "the form's field {name} is a list of something other than choices"
                ));
            }
            Kind::Choice {
                options,
                multiple: true,
                min: property["minItems"].as_u64(),
                max: property["maxItems"].as_u64(),
            }
        }
        _ => {
            return Err(format!(
                "the form's field {name} is of a kind a form cannot hold"
            ))
        }
    };
    if let Kind::Choice { options, .. } = &kind {
        if options.is_empty() {
            return Err(format!("the form's field {name} has no choices"));
        }
    }
    Ok(Field {
        name: name.to_string(),
        title: text("title"),
        description: text("description"),
        kind,
        required,
        default: property.get("default").cloned(),
    })
}

/// How a field is described to the person.
fn describe(field: &Field) -> String {
    let what = match &field.kind {
        Kind::Text { format, min, max } => {
            let mut what = match format.as_deref() {
                Some("email") => "an email address".to_string(),
                Some("uri") => "an address (URL)".to_string(),
                Some("date") => "a date, YYYY-MM-DD".to_string(),
                Some("date-time") => "a date and time, YYYY-MM-DDTHH:MM:SSZ".to_string(),
                _ => "text".to_string(),
            };
            match (min, max) {
                (Some(min), Some(max)) => what.push_str(&format!(", {min} to {max} characters")),
                (Some(min), None) => what.push_str(&format!(", at least {min} characters")),
                (None, Some(max)) => what.push_str(&format!(", at most {max} characters")),
                _ => {}
            }
            what
        }
        Kind::Number { integer, min, max } => {
            let mut what = if *integer {
                "a whole number"
            } else {
                "a number"
            }
            .to_string();
            match (min, max) {
                (Some(min), Some(max)) => what.push_str(&format!(" from {min} to {max}")),
                (Some(min), None) => what.push_str(&format!(", at least {min}")),
                (None, Some(max)) => what.push_str(&format!(", at most {max}")),
                _ => {}
            }
            what
        }
        Kind::Boolean => "yes or no".to_string(),
        Kind::Choice {
            options, multiple, ..
        } => {
            let titles: Vec<&str> = options.iter().map(|(_, t)| t.as_str()).collect();
            if *multiple {
                format!("any of {} (separated by commas)", titles.join(", "))
            } else {
                format!("one of {}", titles.join(", "))
            }
        }
    };
    let mut line = format!("- {}", field.name);
    let mut notes = vec![what];
    if field.required {
        notes.push("needed".into());
    }
    if let Some(default) = &field.default {
        notes.push(format!("{} unless you say otherwise", shown(default)));
    }
    line.push_str(&format!(" ({})", notes.join("; ")));
    if let Some(title) = field.title.as_ref().filter(|t| **t != field.name) {
        line.push_str(&format!(": {title}"));
    }
    if let Some(description) = &field.description {
        line.push_str(&format!(" — {description}"));
    }
    line
}

fn shown(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Array(items) => items.iter().map(shown).collect::<Vec<_>>().join(", "),
        Value::Bool(true) => "yes".into(),
        Value::Bool(false) => "no".into(),
        other => other.to_string(),
    }
}

/// What a form asks, in words, from the server `server`.
pub fn question(server: &str, request: &Elicitation) -> String {
    match request {
        Elicitation::Form { message, fields } => {
            let mut text = format!(
                "The MCP server {server} asks you, through this conversation: \"{message}\"\n"
            );
            if fields.is_empty() {
                text.push_str("Say \"yes\" to agree, \"decline\" to refuse, or \"cancel\".");
                return text;
            }
            text.push_str("It wants:\n");
            for field in fields {
                text.push_str(&describe(field));
                text.push('\n');
            }
            if fields.len() == 1 {
                text.push_str("Reply with the value; ");
            } else {
                text.push_str("Reply with a line `name: value` for each; ");
            }
            text.push_str(
                "\"decline\" refuses, \"cancel\" lets it go. Nothing is sent until you reply.",
            );
            text
        }
        Elicitation::Url { message, url } => {
            let host = url
                .split("://")
                .nth(1)
                .and_then(|rest| rest.split(['/', '?', '#']).next())
                .unwrap_or_default();
            let mut warnings = Vec::new();
            if url.starts_with("http://") {
                warnings.push("it is not HTTPS");
            }
            if host.contains("xn--") {
                warnings.push("its host is written in punycode: check it is the one you mean");
            }
            if host.contains('@') {
                warnings.push("it names a user before its host: the host is what follows the @");
            }
            let mut text = format!(
                "The MCP server {server} asks you to open an address yourself, outside this \
                 conversation: \"{message}\"\nAddress: {url}\nHost: {host}\n"
            );
            if !warnings.is_empty() {
                text.push_str(&format!("Careful: {}.\n", warnings.join("; ")));
            }
            text.push_str(
                "Nothing here opens it. Open it in your browser only if you trust it, then say \
                 \"done\"; \"decline\" refuses, \"cancel\" lets it go.",
            );
            text
        }
    }
}

/// The person's reply, read against the form: the values, or what is
/// wrong with it.
pub fn read_reply(fields: &[Field], reply: &str) -> Result<Map<String, Value>, String> {
    let mut given: Map<String, Value> = Map::new();
    let lines: Vec<&str> = reply
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let mut raw: Vec<(String, String)> = Vec::new();
    if fields.len() == 1
        && !lines.iter().any(|l| {
            l.split_once(':')
                .is_some_and(|(k, _)| k.trim() == fields[0].name)
        })
    {
        raw.push((fields[0].name.clone(), reply.trim().to_string()));
    } else {
        for line in lines {
            let Some((name, value)) = line.split_once(':') else {
                return Err(format!("\"{line}\" is not `name: value`"));
            };
            let name = name.trim().trim_start_matches("- ").to_string();
            raw.push((name, value.trim().to_string()));
        }
    }
    for (name, value) in raw {
        let Some(field) = fields
            .iter()
            .find(|f| f.name == name || f.title.as_deref() == Some(name.as_str()))
        else {
            return Err(format!("the form has no field {name}"));
        };
        given.insert(field.name.clone(), value_of(field, &value)?);
    }
    for field in fields {
        if !given.contains_key(&field.name) {
            match &field.default {
                Some(default) => {
                    given.insert(field.name.clone(), default.clone());
                }
                None if field.required => return Err(format!("{} is needed", field.name)),
                None => {}
            }
        }
    }
    Ok(given)
}

fn value_of(field: &Field, text: &str) -> Result<Value, String> {
    let name = &field.name;
    match &field.kind {
        Kind::Text { format, min, max } => {
            let count = text.chars().count() as u64;
            if min.is_some_and(|m| count < m) || max.is_some_and(|m| count > m) {
                return Err(format!(
                    "{name} has {count} characters, outside what it takes"
                ));
            }
            let ok = match format.as_deref() {
                Some("email") => {
                    let parts: Vec<&str> = text.split('@').collect();
                    parts.len() == 2
                        && !parts[0].is_empty()
                        && parts[1].contains('.')
                        && !text.contains(' ')
                }
                Some("uri") => text.contains("://") && !text.contains(' '),
                Some("date") => is_date(text),
                Some("date-time") => {
                    text.len() >= 16
                        && text.get(..10).is_some_and(is_date)
                        && text.as_bytes()[10] == b'T'
                }
                _ => true,
            };
            if !ok {
                return Err(format!(
                    "{name} is not {}",
                    describe(field)
                        .split('(')
                        .nth(1)
                        .and_then(|s| s.split([';', ')']).next())
                        .unwrap_or("what it takes")
                ));
            }
            Ok(json!(text))
        }
        Kind::Number { integer, min, max } => {
            let number: f64 = text
                .replace(',', "")
                .parse()
                .map_err(|_| format!("{name} is not a number"))?;
            if *integer && number.fract() != 0.0 {
                return Err(format!("{name} is a whole number"));
            }
            if min.is_some_and(|m| number < m) || max.is_some_and(|m| number > m) {
                return Err(format!("{name} is outside what it takes"));
            }
            Ok(if *integer {
                json!(number as i64)
            } else {
                json!(number)
            })
        }
        Kind::Boolean => match text.to_lowercase().as_str() {
            "yes" | "y" | "true" | "on" | "1" => Ok(json!(true)),
            "no" | "n" | "false" | "off" | "0" => Ok(json!(false)),
            _ => Err(format!("{name} is yes or no")),
        },
        Kind::Choice {
            options,
            multiple,
            min,
            max,
        } => {
            let pick = |word: &str| {
                let word = word.trim();
                options
                    .iter()
                    .find(|(value, title)| {
                        value.eq_ignore_ascii_case(word) || title.eq_ignore_ascii_case(word)
                    })
                    .map(|(value, _)| value.clone())
                    .ok_or_else(|| format!("{word} is not one of {name}'s choices"))
            };
            if *multiple {
                let chosen: Vec<String> = text
                    .split(',')
                    .filter(|w| !w.trim().is_empty())
                    .map(pick)
                    .collect::<Result<_, _>>()?;
                let n = chosen.len() as u64;
                if min.is_some_and(|m| n < m) || max.is_some_and(|m| n > m) {
                    return Err(format!("{name} takes a different number of choices"));
                }
                Ok(json!(chosen))
            } else {
                Ok(json!(pick(text)?))
            }
        }
    }
}

fn is_date(text: &str) -> bool {
    let b = text.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

/// What a reply means beyond values: a refusal, letting it go, or agreeing
/// (for a form with no fields, or an address).
fn intent(reply: &str) -> Option<&'static str> {
    match reply
        .trim()
        .trim_end_matches(['.', '!'])
        .to_lowercase()
        .as_str()
    {
        "decline" | "no" | "refuse" | "/decline" => Some("decline"),
        "cancel" | "stop" | "never mind" | "nevermind" => Some("cancel"),
        "yes" | "ok" | "okay" | "done" | "opened" | "accept" | "agree" => Some("accept"),
        _ => None,
    }
}

/// Asks the person what the server `server` wants (`params`, an
/// `elicitation/create` request's) and returns the `ElicitResult`: the
/// values they gave, their refusal, or `cancel` when no answer came.
pub fn elicit(asker: &dyn Asker, server: &str, params: &Value) -> Value {
    let request = match Elicitation::parse(params) {
        Ok(request) => request,
        Err(why) => {
            asker.tell(&format!("The MCP server {server} asked for something this client cannot ask you ({why}); it was declined."));
            return json!({"action": "decline"});
        }
    };
    if request.asks_for_a_secret() {
        asker.tell(&format!(
            "The MCP server {server} asked for a password, a key or a card number in a form, \
             which servers must not do; it was declined without asking you."
        ));
        return json!({"action": "decline"});
    }
    let mut asked = question(server, &request);
    // One more chance after a reply the form does not take.
    for _ in 0..2 {
        let Some(reply) = asker.ask(&asked) else {
            return json!({"action": "cancel"});
        };
        match intent(&reply) {
            Some("decline") => return json!({"action": "decline"}),
            Some("cancel") => return json!({"action": "cancel"}),
            _ => {}
        }
        match &request {
            Elicitation::Url { .. } => {
                if intent(&reply) == Some("accept") {
                    return json!({"action": "accept"});
                }
                asked = "Say \"done\" once you opened it, \"decline\" or \"cancel\".".into();
            }
            Elicitation::Form { fields, .. } if fields.is_empty() => {
                if intent(&reply) == Some("accept") {
                    return json!({"action": "accept", "content": {}});
                }
                asked = "Say \"yes\", \"decline\" or \"cancel\".".into();
            }
            Elicitation::Form { fields, .. } => match read_reply(fields, &reply) {
                Ok(content) => return json!({"action": "accept", "content": content}),
                Err(why) => {
                    asked = format!("{why}. Try again, or say \"decline\" or \"cancel\".");
                }
            },
        }
    }
    json!({"action": "cancel"})
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// A person who replies in turn, and what they were asked.
    #[derive(Default)]
    struct Person {
        replies: Mutex<VecDeque<&'static str>>,
        asked: Mutex<Vec<String>>,
        told: Mutex<Vec<String>>,
    }

    impl Asker for Person {
        fn ask(&self, question: &str) -> Option<String> {
            self.asked.lock().unwrap().push(question.to_string());
            self.replies.lock().unwrap().pop_front().map(str::to_string)
        }
        fn tell(&self, text: &str) {
            self.told.lock().unwrap().push(text.to_string());
        }
    }

    fn person(replies: &[&'static str]) -> Person {
        Person {
            replies: Mutex::new(replies.iter().copied().collect()),
            ..Person::default()
        }
    }

    fn form() -> Value {
        json!({"mode": "form", "message": "Where should the note go?", "requestedSchema": {
            "type": "object",
            "properties": {
                "folder": {"type": "string", "title": "Folder", "minLength": 1},
                "urgent": {"type": "boolean", "default": false},
                "copies": {"type": "integer", "minimum": 1, "maximum": 3},
                "color": {"type": "string", "oneOf": [{"const": "#f00", "title": "Red"}, {"const": "#00f", "title": "Blue"}]},
                "tags": {"type": "array", "items": {"type": "string", "enum": ["work", "home"]}, "maxItems": 2},
                "email": {"type": "string", "format": "email"}
            },
            "required": ["folder", "copies"]
        }})
    }

    #[test]
    fn a_form_is_asked_in_words_and_its_reply_checked() {
        let someone = person(&[
            "folder: Inbox\ncopies: 9",
            "folder: Inbox\ncopies: 2\ncolor: blue\ntags: work, home\nemail: ana@example.org",
        ]);
        let result = elicit(&someone, "notes", &form());
        assert_eq!(result["action"], "accept");
        assert_eq!(
            result["content"],
            json!({"folder": "Inbox", "copies": 2, "color": "#00f", "tags": ["work", "home"], "email": "ana@example.org", "urgent": false})
        );
        let asked = someone.asked.lock().unwrap().clone();
        assert!(asked[0].starts_with("The MCP server notes asks you, through this conversation: \"Where should the note go?\""), "{}", asked[0]);
        assert!(
            asked[0].contains("- copies (a whole number from 1 to 3; needed)"),
            "{}",
            asked[0]
        );
        assert!(
            asked[0].contains("- color (one of Red, Blue)"),
            "{}",
            asked[0]
        );
        assert!(
            asked[0].contains("- urgent (yes or no; no unless you say otherwise)"),
            "{}",
            asked[0]
        );
        // The first reply was outside the form: asked again, saying why.
        assert!(
            asked[1].starts_with("copies is outside what it takes"),
            "{}",
            asked[1]
        );
    }

    #[test]
    fn refusals_silence_secrets_and_addresses() {
        assert_eq!(
            elicit(&person(&["decline"]), "s", &form())["action"],
            "decline"
        );
        assert_eq!(
            elicit(&person(&[]), "s", &form())["action"],
            "cancel",
            "no reply in time"
        );
        assert_eq!(
            elicit(&person(&["what?", "huh"]), "s", &form())["action"],
            "cancel"
        );
        // A form asking for a password is declined without asking.
        let secret = json!({"message": "Log in", "requestedSchema": {"type": "object", "properties": {"api_key": {"type": "string"}}}});
        let someone = person(&["sk-123"]);
        assert_eq!(elicit(&someone, "s", &secret)["action"], "decline");
        assert!(someone.asked.lock().unwrap().is_empty());
        assert!(someone.told.lock().unwrap()[0].contains("must not do"));
        // So is a form of one plain field whose message asks for one.
        let worded = json!({"message": "Paste your GitHub token", "requestedSchema": {"type": "object", "properties": {"value": {"type": "string"}}}});
        let someone = person(&["ghp_x"]);
        assert_eq!(elicit(&someone, "s", &worded)["action"], "decline");
        assert!(someone.asked.lock().unwrap().is_empty());
        // One field: the value alone.
        let one = json!({"message": "Your name?", "requestedSchema": {"type": "object", "properties": {"name": {"type": "string"}}, "required": ["name"]}});
        assert_eq!(
            elicit(&person(&["Ana: of Lisbon"]), "s", &one)["content"]["name"],
            "Ana: of Lisbon"
        );
        // An address: shown, host apart, never opened here.
        let url = json!({"mode": "url", "message": "Connect your calendar", "url": "http://xn--80ak6aa92e.com/connect"});
        let someone = person(&["done"]);
        assert_eq!(elicit(&someone, "cal", &url), json!({"action": "accept"}));
        let asked = someone.asked.lock().unwrap()[0].clone();
        assert!(
            asked.contains("Host: xn--80ak6aa92e.com")
                && asked.contains("not HTTPS")
                && asked.contains("punycode"),
            "{asked}"
        );
        assert!(Elicitation::parse(
            &json!({"mode": "url", "message": "x", "url": "javascript:alert(1)"})
        )
        .is_err());
        let nested = json!({"message": "x", "requestedSchema": {"type": "object", "properties": {"a": {"type": "object"}}}});
        assert_eq!(elicit(&person(&["yes"]), "s", &nested)["action"], "decline");
    }

    #[test]
    fn the_responder_says_what_it_can_and_counts_sampling() {
        struct Echo;
        impl Sampler for Echo {
            fn sample(&self, _server: &str, params: &Value) -> Result<(Value, Usage), String> {
                Ok((
                    json!({"role": "assistant", "content": {"type": "text", "text": params["messages"][0]["content"]["text"]}, "model": "m", "stopReason": "endTurn"}),
                    Usage {
                        input: 3,
                        output: 4,
                        ..Usage::default()
                    },
                ))
            }
        }
        let none = Responder::default();
        assert_eq!(none.capabilities(), json!({}));
        assert_eq!(
            none.answer("s", "elicitation/create", &json!({}))
                .unwrap_err()
                .0,
            -32601
        );
        assert_eq!(
            none.respond("s", &json!({"id": 7, "method": "ping"})),
            json!({"jsonrpc": "2.0", "id": 7, "result": {}})
        );
        let both = Responder {
            asker: Some(Arc::new(person(&[]))),
            sampler: Some(Arc::new(Echo)),
            ..Responder::default()
        };
        assert_eq!(
            both.capabilities(),
            json!({"elicitation": {"form": {}, "url": {}}, "sampling": {}})
        );
        let sampled = both
            .answer(
                "s",
                "sampling/createMessage",
                &json!({"messages": [{"role": "user", "content": {"type": "text", "text": "hi"}}]}),
            )
            .unwrap();
        assert_eq!(sampled["content"]["text"], "hi");
        let spent = both.take_spent().unwrap();
        assert_eq!((spent.input, spent.output), (3, 4));
        assert!(both.take_spent().is_none());
    }
}
