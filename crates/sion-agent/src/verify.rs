//! Checking that a task is done, apart from the model that did it.
//!
//! A model that says it has finished may not have: tests it did not run, a
//! file it did not write. With a verifier (a command the operator gives,
//! run where the agent works), the runtime checks each time the model says
//! it is done. A check that fails is shown to the model, which goes on;
//! after [`crate::Limits::verify_attempts`] failed checks the run ends with
//! the best attempt it made ([`crate::Ended::Unverified`]), not with a claim
//! of success. And the model may say, with [`CANNOT_COMPLETE`], that it
//! cannot do the task: an honest end rather than an answer made to look
//! finished ([`crate::Ended::CannotComplete`]).
//!
//! A verifier's output may end with a score (`score: 0.8`, from 0 to 1),
//! for checks that pass by degrees; the best attempt is the one with the
//! highest score, the last of them on a tie.

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::model::ToolSpec;

/// The meta-tool with which the model says it cannot complete the task.
pub const CANNOT_COMPLETE: &str = "cannot_complete";

/// The name of a check in a session's trace.
pub const CHECK: &str = "check";

/// What the model is told a failed check reported begins with this.
pub const FAILED: &str = "[The check of your answer failed";

/// The most of a verifier's report the model is shown: its end.
pub const REPORT_CHARS: usize = 4_000;

/// What a check found.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Verdict {
    /// Whether it passed.
    pub passed: bool,
    /// Its score, from 0 to 1, if it gave one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    /// What it reported (a command's output).
    pub report: String,
}

impl Verdict {
    /// A verdict from a command that exited with `code` and printed
    /// `output`: it passed if the code is 0; its score is the last line's
    /// `score: <number>`, if there is one.
    pub fn of_command(code: Option<i32>, output: &str) -> Verdict {
        Verdict {
            passed: code == Some(0),
            score: score_of(output),
            report: output.to_string(),
        }
    }

    /// How good it is, for choosing the best attempt: its score, or 1 if it
    /// passed and 0 if it did not.
    pub fn worth(&self) -> f64 {
        self.score
            .unwrap_or(if self.passed { 1.0 } else { 0.0 })
            .clamp(0.0, 1.0)
    }
}

/// The score a check's output ends with: a last line `score: 0.8` (or
/// `score 0.8`), from 0 to 1.
pub fn score_of(output: &str) -> Option<f64> {
    let last = output.lines().rev().find(|l| !l.trim().is_empty())?.trim();
    let rest = last
        .strip_prefix("score:")
        .or_else(|| last.strip_prefix("score "))?;
    let score: f64 = rest.trim().parse().ok()?;
    (0.0..=1.0).contains(&score).then_some(score)
}

/// The tool the model says it cannot complete the task with.
pub fn spec() -> ToolSpec {
    ToolSpec {
        name: CANNOT_COMPLETE.into(),
        description: "Say that you cannot complete the task, and why. The run ends there, as \
                      not done: better than an answer that only looks finished. Use it when what \
                      the task needs is out of your reach (a permission, a file, a service), not \
                      to stop early."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {"why": {"type": "string"}},
            "required": ["why"]
        }),
    }
}

/// What the model is told when the check after its answer failed: what
/// the check reported (its end), and what it may do.
pub fn failed(verdict: &Verdict, attempt: u32, attempts: u32) -> String {
    let report = verdict.report.trim_end();
    let shown: String = if report.chars().count() > REPORT_CHARS {
        let skip = report.chars().count() - REPORT_CHARS;
        format!("…{}", report.chars().skip(skip).collect::<String>())
    } else {
        report.to_string()
    };
    let score = verdict
        .score
        .map(|s| format!(" (score {s})"))
        .unwrap_or_default();
    format!(
        "{FAILED}{score}, attempt {attempt} of {attempts}. It reported:\n{shown}\nFix what it \
         reports and finish again, or, if the task cannot be done, say so with \
         {CANNOT_COMPLETE}.]"
    )
}

/// The last line of a check's report that says something, shortened.
pub fn last_line(report: &str) -> String {
    let line = report
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("it reported nothing");
    if line.chars().count() > 200 {
        format!("{}…", line.chars().take(200).collect::<String>())
    } else {
        line.to_string()
    }
}

/// The attempts of a run: how many were checked, and the best of them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Attempts {
    /// Checks made.
    pub checked: u32,
    /// The best attempt so far.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub best: Option<Attempt>,
}

/// An attempt that was checked.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Attempt {
    /// Which it was, from 1.
    pub number: u32,
    /// What the model answered.
    pub text: String,
    /// What the check found.
    pub verdict: Verdict,
}

impl Attempts {
    /// Whether no answer was checked.
    pub fn is_empty(&self) -> bool {
        self.checked == 0 && self.best.is_none()
    }

    /// Records the check of an answer: the best is the one worth most, the
    /// later of two worth the same.
    pub fn record(&mut self, text: &str, verdict: Verdict) {
        self.checked += 1;
        let better = self
            .best
            .as_ref()
            .map_or(true, |best| verdict.worth() >= best.verdict.worth());
        if better {
            self.best = Some(Attempt {
                number: self.checked,
                text: text.to_string(),
                verdict,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_passes_on_exit_zero_and_may_score() {
        let passed = Verdict::of_command(Some(0), "12 tests passed\nscore: 1\n");
        assert!(passed.passed && passed.score == Some(1.0));
        let failed = Verdict::of_command(Some(1), "2 of 12 failed\nscore 0.83");
        assert!(!failed.passed && failed.score == Some(0.83));
        assert_eq!(failed.worth(), 0.83);
        let killed = Verdict::of_command(None, "timed out");
        assert!(!killed.passed && killed.score.is_none() && killed.worth() == 0.0);
        assert_eq!(score_of("score: 7"), None, "a score is from 0 to 1");
        assert_eq!(score_of("the score: 0.5 was printed earlier\nok"), None);
    }

    #[test]
    fn the_best_attempt_is_the_one_worth_most_and_the_later_on_a_tie() {
        let mut attempts = Attempts::default();
        attempts.record("first", Verdict::of_command(Some(1), "score: 0.5"));
        attempts.record("second", Verdict::of_command(Some(1), "score: 0.9"));
        attempts.record("third", Verdict::of_command(Some(1), "score: 0.4"));
        assert_eq!(attempts.checked, 3);
        let best = attempts.best.as_ref().unwrap();
        assert_eq!((best.number, best.text.as_str()), (2, "second"));
        attempts.record("fourth", Verdict::of_command(Some(1), "score: 0.9"));
        assert_eq!(attempts.best.unwrap().text, "fourth");
    }

    #[test]
    fn a_report_ends_with_its_last_line() {
        assert_eq!(
            last_line("1 failed\n  test add ... FAILED  \n\n"),
            "test add ... FAILED"
        );
        assert_eq!(last_line(""), "it reported nothing");
        assert!(last_line(&"x".repeat(500)).chars().count() <= 201);
        assert!(Attempts::default().is_empty());
    }

    #[test]
    fn the_model_is_shown_the_end_of_a_long_report() {
        let long = format!("{}\nthe last line", "x".repeat(10_000));
        let note = failed(&Verdict::of_command(Some(1), &long), 1, 3);
        assert!(note.contains("attempt 1 of 3") && note.contains("the last line"));
        assert!(note.contains(CANNOT_COMPLETE));
        assert!(note.chars().count() < REPORT_CHARS + 400);
    }
}
