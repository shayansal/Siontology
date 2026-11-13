//! What the agent expects of a call, said before it is made and checked
//! after.
//!
//! An agent that only acts learns from a task's end, pass or fail, and
//! little of why. One that says first what it expects learns from every
//! call: a result that is not the one it said is a place where what it
//! believed of its tools, its files or the world was wrong, whether or not
//! the task then went well. That is the signal a planner corrects itself
//! by: predict, act, compare, and change what led to the prediction when
//! the two differ.
//!
//! With [`crate::session::Limits::expect`], every tool the model is shown
//! takes one more property, [`EXPECT`], which the tool itself never sees
//! ([`offered`], [`taken`]): a sentence saying what the call will do, and
//! what can be checked of it without a model, whether the call fails and
//! what its result holds ([`Expectation`]). The runtime compares that with
//! what happened ([`Expectation::judge`]). It says nothing where the two
//! agree. Where they differ it tells the model so in the call's result
//! ([`note`]) and keeps the miss with the session ([`Expected`]), for the
//! session's record and for whatever improves the agent from its runs.
//!
//! What is checked is narrow on purpose: whether the call failed, and
//! whether words are in its result. The sentence is the agent's own and is
//! kept as it wrote it, not judged. An agent can therefore always be right
//! by expecting little, which is why the count of expectations that named
//! something to check is kept beside the count of those that held
//! ([`Expected::sharp`]), and why neither count decides anything here.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::model::ToolSpec;
use crate::tools::Outcome;

/// The property a call's expectation is given in.
pub const EXPECT: &str = "expect";

/// How the runtime's note of a miss begins ([`note`]).
pub const NOTE: &str = "[Not as you expected. You expected:";

/// The misses a session keeps: the last of them.
pub const MISSED_KEPT: usize = 50;

/// The most words an expectation may name for the result to hold, and the
/// longest each may be.
pub const MAX_CONTAINS: usize = 8;

/// The longest a word to look for, or the sentence, is kept at.
pub const MAX_TEXT: usize = 300;

/// What the model is told of expectations, after its system prompt.
pub const EXPECTING: &str = "\n\nExpectations. Every tool takes one more, optional input, \
     `expect`: what you expect the call to do, said before you make it. Give `outcome`, a \
     sentence, and what can be checked of it: `ok` (false if you expect the call to fail; true \
     if left out) and `contains` (words its result will hold). Say it for a call that changes \
     something, or whose result you are not sure of, and leave it out where there is nothing to \
     predict. The runtime compares it with what happened and tells you only when it did not \
     happen as you said. Then something you believed was wrong: work out what before you go \
     on, rather than making the same call again.";

/// What the agent said a call would do.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Expectation {
    /// What it expects, in its own words.
    pub outcome: String,
    /// Whether it expects the call not to fail.
    pub ok: bool,
    /// Words it expects the result to hold, case and spacing aside, each
    /// on one line.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub contains: Vec<String>,
}

/// `text` on one line: each run of space, line breaks included, a single
/// space. What the agent writes is kept and shown this way, so that none
/// of it can pass for a line of its own where it is quoted.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `text` on one line, cut to [`MAX_TEXT`] characters.
fn clipped(text: &str) -> String {
    let line = one_line(text);
    if line.chars().count() <= MAX_TEXT {
        return line;
    }
    let cut: String = line.chars().take(MAX_TEXT).collect();
    format!("{cut}…")
}

impl Expectation {
    /// The expectation `value` gives, read as kindly as it can be: the
    /// object described to the model, or a bare sentence. None where it
    /// says nothing that can be kept (no sentence and nothing to check).
    pub fn of(value: &Value) -> Option<Expectation> {
        if let Some(sentence) = value.as_str() {
            let outcome = clipped(sentence);
            return (!outcome.is_empty()).then_some(Expectation {
                outcome,
                ok: true,
                contains: Vec::new(),
            });
        }
        let object = value.as_object()?;
        let ok = match object.get("ok") {
            Some(Value::Bool(ok)) => *ok,
            Some(Value::String(said)) => !said.trim().eq_ignore_ascii_case("false"),
            _ => true,
        };
        let words: Vec<&str> = match object.get("contains") {
            Some(Value::String(one)) => vec![one.as_str()],
            Some(Value::Array(many)) => many.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        };
        let contains: Vec<String> = words
            .into_iter()
            .map(one_line)
            .filter(|word| !word.is_empty())
            .take(MAX_CONTAINS)
            .map(|word| word.chars().take(MAX_TEXT).collect())
            .collect();
        let outcome = clipped(object.get("outcome").and_then(Value::as_str).unwrap_or(""));
        if outcome.is_empty() && contains.is_empty() && ok {
            return None;
        }
        Some(Expectation {
            outcome,
            ok,
            contains,
        })
    }

    /// Whether it names something to check beyond the call not failing:
    /// that it will fail, or words its result will hold.
    pub fn sharp(&self) -> bool {
        !self.ok || !self.contains.is_empty()
    }

    /// How `outcome` differs from what was expected, each difference in
    /// words; none where it went as said. `None` where it cannot be told:
    /// the call may or may not have taken effect.
    ///
    /// The words are looked for in what the model is shown of the result.
    /// Where the result is kept from it (`withheld`, content from outside
    /// held in quarantine), only whether the call failed is compared: what
    /// the model may not read is not answered about either.
    pub fn judge(&self, outcome: &Outcome, withheld: bool) -> Option<Vec<String>> {
        if !outcome.certain {
            return None;
        }
        // A call that failed where it was expected not to, or the other way
        // round: that is the difference. The words were said of a result of
        // the other kind, and are not looked for in this one.
        match (self.ok, outcome.is_error) {
            (true, true) => return Some(vec!["the call failed".to_string()]),
            (false, false) => return Some(vec!["the call did not fail".to_string()]),
            _ => {}
        }
        let mut differs = Vec::new();
        if !withheld {
            // Case and spacing aside: a result's line breaks and indents are
            // not what was predicted.
            let said = one_line(&outcome.content).to_lowercase();
            for word in &self.contains {
                if !said.contains(&word.to_lowercase()) {
                    differs.push(format!("the result does not hold \"{word}\""));
                }
            }
        }
        Some(differs)
    }

    /// The sentence, or where there is none what was to be checked.
    pub fn said(&self) -> String {
        if !self.outcome.is_empty() {
            return self.outcome.clone();
        }
        let mut parts = Vec::new();
        if !self.ok {
            parts.push("the call fails".to_string());
        }
        if !self.contains.is_empty() {
            parts.push(format!(
                "the result holds {}",
                self.contains
                    .iter()
                    .map(|word| format!("\"{word}\""))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        parts.join(" and ")
    }
}

/// The schema of the property, as every tool is given it: small, since it
/// is said once for each tool, with the rest said once ([`EXPECTING`]).
pub fn property() -> Value {
    json!({
        "type": "object",
        "description": "Optional: what you expect this call to do (see Expectations).",
        "properties": {
            "outcome": {"type": "string"},
            "ok": {"type": "boolean"},
            "contains": {"type": "array", "items": {"type": "string"}}
        }
    })
}

/// Whether the tool `spec` describes can be given the property: its input
/// is an object with properties of its own, none of them called
/// [`EXPECT`].
pub fn fits(spec: &ToolSpec) -> bool {
    spec.input_schema["type"] == "object"
        && match spec.input_schema.get("properties") {
            None => true,
            Some(properties) => properties
                .as_object()
                .is_some_and(|all| !all.contains_key(EXPECT)),
        }
}

/// `spec` as the model is shown it when expectations are on: with the
/// property, where it [`fits`].
pub fn offered(mut spec: ToolSpec) -> ToolSpec {
    if !fits(&spec) {
        return spec;
    }
    if let Some(schema) = spec.input_schema.as_object_mut() {
        let properties = schema
            .entry("properties")
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
        if let Some(properties) = properties.as_object_mut() {
            properties.insert(EXPECT.to_string(), property());
        }
    }
    spec
}

/// `input` as its tool is given it, without the expectation, and the
/// expectation it carried, if any that can be kept.
pub fn taken(input: &Value) -> (Value, Option<Expectation>) {
    let Some(object) = input.as_object() else {
        return (input.clone(), None);
    };
    let Some(said) = object.get(EXPECT) else {
        return (input.clone(), None);
    };
    let expectation = Expectation::of(said);
    let mut without = object.clone();
    without.remove(EXPECT);
    (Value::Object(without), expectation)
}

/// `input` as a record of a session's calls reads it: without the
/// expectation, where the session's calls may carry one (`expecting`,
/// [`crate::session::State::expecting`]).
pub fn without(input: &Value, expecting: bool) -> Value {
    if expecting {
        taken(input).0
    } else {
        input.clone()
    }
}

/// `result` as its tool gave it: without the runtime's note of a miss.
pub fn without_note(result: &str) -> &str {
    match result.rfind(&format!("\n{NOTE}")) {
        Some(at) if result.ends_with(".]") => &result[..at],
        _ => result,
    }
}

/// The runtime's note of a miss, after a call's result: what was expected,
/// and how what happened differs.
pub fn note(expectation: &Expectation, differs: &[String]) -> String {
    format!(
        "\n{NOTE} {}. What happened: {}.]",
        expectation.said().trim_end_matches('.'),
        differs.join("; ")
    )
}

/// A call that did not go as the agent said it would.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Missed {
    /// The tool called.
    pub tool: String,
    /// What it expected, in its own words.
    pub expected: String,
    /// How what happened differs.
    pub happened: String,
}

impl Missed {
    /// The miss on one line.
    pub fn line(&self) -> String {
        format!(
            "{}: expected \"{}\"; {}",
            self.tool, self.expected, self.happened
        )
    }
}

/// What a session's agent said it expected of its calls, and how that
/// went.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Expected {
    /// The calls it stated an expectation for that could be compared.
    #[serde(default)]
    pub stated: u64,
    /// Of those, the ones that went as it said.
    #[serde(default)]
    pub held: u64,
    /// Of those stated, the ones that named something to check beyond the
    /// call not failing ([`Expectation::sharp`]).
    #[serde(default)]
    pub sharp: u64,
    /// The ones that did not go as said, oldest first: the last
    /// [`MISSED_KEPT`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missed: Vec<Missed>,
}

impl Expected {
    /// Whether nothing was stated.
    pub fn is_empty(&self) -> bool {
        self.stated == 0 && self.missed.is_empty()
    }

    /// Counts a call of `tool` that was compared with `expectation`:
    /// `differs` says how it went otherwise, and is empty where it held.
    pub fn count(&mut self, tool: &str, expectation: &Expectation, differs: &[String]) {
        self.stated += 1;
        if expectation.sharp() {
            self.sharp += 1;
        }
        if differs.is_empty() {
            self.held += 1;
            return;
        }
        self.missed.push(Missed {
            // The name is the model's where it called a tool by name, or
            // one there is none of: kept on one line like its other words.
            tool: clipped(tool).chars().take(80).collect(),
            expected: expectation.said(),
            happened: differs.join("; "),
        });
        if self.missed.len() > MISSED_KEPT {
            let over = self.missed.len() - MISSED_KEPT;
            self.missed.drain(..over);
        }
    }

    /// The counts in a line, for a person: none where nothing was stated.
    pub fn line(&self) -> Option<String> {
        if self.stated == 0 {
            return None;
        }
        Some(format!(
            "{} of {} calls went as it said they would ({} of its expectations named \
             something to check)",
            self.held, self.stated, self.sharp
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(schema: Value) -> ToolSpec {
        ToolSpec {
            name: "t".into(),
            description: "a tool".into(),
            input_schema: schema,
        }
    }

    #[test]
    fn an_expectation_is_read_kindly_and_kept_small() {
        let full = Expectation::of(&json!({
            "outcome": "  the tests\npass ",
            "ok": true,
            "contains": ["0 failed", "", "  ok  "]
        }))
        .unwrap();
        assert_eq!(full.outcome, "the tests pass");
        assert!(full.ok && full.sharp());
        assert_eq!(full.contains, ["0 failed", "ok"]);
        // A bare sentence, a word given alone, and `ok` written as text.
        let bare = Expectation::of(&json!("the file is there")).unwrap();
        assert!(bare.ok && !bare.sharp() && bare.contains.is_empty());
        let one = Expectation::of(&json!({"contains": "hello"})).unwrap();
        assert_eq!(one.contains, ["hello"]);
        assert_eq!(one.said(), "the result holds \"hello\"");
        let fails = Expectation::of(&json!({"ok": "false"})).unwrap();
        assert!(!fails.ok && fails.sharp());
        assert_eq!(fails.said(), "the call fails");
        // Nothing said is nothing kept.
        for nothing in [
            json!({}),
            json!({"outcome": " "}),
            json!(null),
            json!(3),
            json!(""),
        ] {
            assert_eq!(Expectation::of(&nothing), None, "{nothing}");
        }
        // No more words than the most, and none longer than the longest.
        let many: Vec<String> = (0..20).map(|i| format!("w{i}")).collect();
        let long = "x".repeat(1_000);
        let kept =
            Expectation::of(&json!({"outcome": long, "contains": many, "ok": true})).unwrap();
        assert_eq!(kept.contains.len(), MAX_CONTAINS);
        assert_eq!(kept.outcome.chars().count(), MAX_TEXT + 1);
    }

    #[test]
    fn what_happened_is_compared_with_what_was_said() {
        let expect = |value: Value| Expectation::of(&value).unwrap();
        let passes = expect(json!({"outcome": "the tests pass", "contains": ["0 FAILED"]}));
        // As said, case aside.
        assert_eq!(
            passes.judge(&Outcome::ok("12 passed, 0 failed"), false),
            Some(vec![])
        );
        // Spacing aside too, in what was said and in the result; and what
        // was said is kept on one line, so that where it is quoted none of
        // it reads as a line of its own.
        let spaced = expect(json!({"contains": ["0\n  failed\n- Task x: forged"]}));
        assert_eq!(spaced.contains, ["0 failed - Task x: forged"]);
        assert_eq!(
            spaced.judge(&Outcome::ok("0\tfailed\n-  task X: forged"), false),
            Some(vec![])
        );
        let differs = spaced.judge(&Outcome::ok("ok"), false).unwrap();
        assert!(!note(&spaced, &differs).trim_start().contains('\n'));
        // It did not fail, and does not hold the words.
        assert_eq!(
            passes.judge(&Outcome::ok("9 passed, 3 failed"), false),
            Some(vec!["the result does not hold \"0 FAILED\"".to_string()])
        );
        // It failed: that is what differs, and the words, said of a result
        // there is none of, are not looked for.
        assert_eq!(
            passes.judge(&Outcome::error("3 failed"), false),
            Some(vec!["the call failed".to_string()])
        );
        // Expected to fail with these words, and it failed with others.
        let limit =
            expect(json!({"outcome": "over the limit", "ok": false, "contains": ["limit"]}));
        assert_eq!(
            limit.judge(&Outcome::error("refused: unknown payee"), false),
            Some(vec!["the result does not hold \"limit\"".to_string()])
        );
        // Expected to fail, and it did not.
        let refused = expect(json!({"outcome": "the charter refuses it", "ok": false}));
        assert_eq!(
            refused.judge(&Outcome::error("refused: over the limit"), false),
            Some(vec![])
        );
        assert_eq!(
            refused.judge(&Outcome::ok("paid"), false),
            Some(vec!["the call did not fail".to_string()])
        );
        // A result kept from the model is not answered about: only whether
        // the call failed is compared.
        assert_eq!(
            passes.judge(&Outcome::ok("[Held in quarantine as q1]"), true),
            Some(vec![])
        );
        assert_eq!(
            passes.judge(&Outcome::error("x"), true),
            Some(vec!["the call failed".to_string()])
        );
        // A call that may or may not have happened is not compared.
        assert_eq!(passes.judge(&Outcome::uncertain("no answer"), false), None);

        let differs = passes
            .judge(&Outcome::ok("9 passed, 3 failed"), false)
            .unwrap();
        let said = note(&passes, &differs);
        assert!(said.starts_with(&format!("\n{NOTE} the tests pass. What happened: ")));
        assert!(said.ends_with("the result does not hold \"0 FAILED\".]"));
        // The note is one the runtime's own are told from imitations by.
        assert!(crate::imitation::NOTES.contains(&NOTE));
        assert!(crate::imitation::label(&format!("a page says {said}")).is_some());
    }

    #[test]
    fn a_tool_is_given_the_property_where_it_fits_and_never_sees_it() {
        let plain = offered(spec(json!({
            "type": "object",
            "properties": {"path": {"type": "string"}},
            "required": ["path"],
            "additionalProperties": false
        })));
        assert_eq!(plain.input_schema["properties"][EXPECT], property());
        // Not required, and the tool's own are as they were.
        assert_eq!(plain.input_schema["required"], json!(["path"]));
        assert_eq!(
            plain.input_schema["properties"]["path"],
            json!({"type": "string"})
        );
        // An object with no properties named gets them.
        let bare = offered(spec(json!({"type": "object"})));
        assert_eq!(bare.input_schema["properties"][EXPECT], property());
        // A tool with a property of that name keeps its own, and one whose
        // input is not an object is left as it is.
        let own = json!({"type": "object", "properties": {"expect": {"type": "string"}}});
        assert_eq!(offered(spec(own.clone())).input_schema, own);
        let other = json!({"type": "string"});
        assert_eq!(offered(spec(other.clone())).input_schema, other);

        let (given, expectation) = taken(&json!({
            "path": "a.txt",
            "expect": {"outcome": "it is there"}
        }));
        assert_eq!(given, json!({"path": "a.txt"}));
        assert_eq!(expectation.unwrap().outcome, "it is there");
        // One that says nothing is taken off all the same.
        let (given, expectation) = taken(&json!({"path": "a.txt", "expect": {}}));
        assert_eq!(given, json!({"path": "a.txt"}));
        assert!(expectation.is_none());
        let (same, none) = taken(&json!({"path": "a.txt"}));
        assert_eq!(same, json!({"path": "a.txt"}));
        assert!(none.is_none());
        // A record of a session's calls reads them as they were made, and
        // a result as its tool gave it.
        let call = json!({"path": "a.txt", "expect": "it is there"});
        assert_eq!(without(&call, true), json!({"path": "a.txt"}));
        assert_eq!(without(&call, false), call);
        let expectation = Expectation::of(&json!("it is there")).unwrap();
        let noted = format!(
            "no such file{}",
            note(&expectation, &["the call failed".to_string()])
        );
        assert_eq!(without_note(&noted), "no such file");
        assert_eq!(without_note("no such file"), "no such file");
    }

    #[test]
    fn a_session_counts_what_held_and_keeps_the_last_misses() {
        let mut expected = Expected::default();
        assert!(expected.is_empty() && expected.line().is_none());
        let plain = Expectation::of(&json!("it works")).unwrap();
        let sharp =
            Expectation::of(&json!({"outcome": "it says hello", "contains": ["hello"]})).unwrap();
        expected.count("shell", &plain, &[]);
        expected.count(
            "shell",
            &sharp,
            &["the result does not hold \"hello\"".into()],
        );
        assert_eq!((expected.stated, expected.held, expected.sharp), (2, 1, 1));
        assert_eq!(
            expected.missed[0].line(),
            "shell: expected \"it says hello\"; the result does not hold \"hello\""
        );
        assert_eq!(
            expected.line().unwrap(),
            "1 of 2 calls went as it said they would (1 of its expectations named something to \
             check)"
        );
        for i in 0..(MISSED_KEPT + 5) {
            expected.count(&format!("t{i}"), &sharp, &["the call failed".into()]);
        }
        assert_eq!(expected.missed.len(), MISSED_KEPT);
        assert_eq!(
            expected.missed.last().unwrap().tool,
            format!("t{}", MISSED_KEPT + 4)
        );
        // A name the model made up is kept on one line, and short.
        expected.count(
            &format!("x\n- Task y: {}", "z".repeat(200)),
            &sharp,
            &["the call failed".into()],
        );
        let line = expected.missed.last().unwrap().line();
        assert!(line.starts_with("x - Task y: zzz") && !line.contains('\n'));
        assert!(expected.missed.last().unwrap().tool.chars().count() <= 80);
    }
}
