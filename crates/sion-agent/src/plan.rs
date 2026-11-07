//! A plan the agent keeps, and the end it is held to.
//!
//! With [`PLAN_UPDATE`] the agent writes down the steps of a task and where
//! each stands, the whole plan each time, as Codex's `update_plan` and
//! Claude Code's to-do list have it. The plan is the session's: shown to
//! whoever watches the run, kept in every summary when the conversation is
//! compacted, and looked at when the agent says it is done. With steps still
//! open, it is asked once to do them or to drop them, saying why
//! ([`reminder`]).
//!
//! A step is done only with how it was checked (the test that passed, the
//! output that showed it), and dropped only with why. A step open in the
//! last plan cannot be left out of the next without a word: it is kept, or
//! dropped with a reason.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::model::ToolSpec;

/// The tool with which the agent writes its plan.
pub const PLAN_UPDATE: &str = "plan_update";

/// The most steps a plan may have.
pub const MAX_STEPS: usize = 30;

/// The most characters a step, or what it says of how it was checked or
/// why it was dropped, may have.
pub const MAX_CHARS: usize = 300;

/// Where a step stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Not begun.
    Pending,
    /// Being done now: one step at a time.
    Active,
    /// Done, and checked.
    Done,
    /// Not to be done, for a reason given.
    Dropped,
}

impl Status {
    /// The status `text` names, in the words models use for it.
    fn parse(text: &str) -> Option<Status> {
        let word = text.trim().to_ascii_lowercase().replace([' ', '-'], "_");
        match word.as_str() {
            "pending" | "todo" | "to_do" | "not_started" | "open" => Some(Status::Pending),
            "active" | "in_progress" | "doing" | "started" | "current" => Some(Status::Active),
            "done" | "completed" | "complete" | "finished" => Some(Status::Done),
            "dropped" | "cancelled" | "canceled" | "skipped" | "abandoned" => Some(Status::Dropped),
            _ => None,
        }
    }

    /// Whether a step so is still to be done.
    pub fn is_open(self) -> bool {
        matches!(self, Status::Pending | Status::Active)
    }

    fn mark(self) -> &'static str {
        match self {
            Status::Pending => "[ ]",
            Status::Active => "[>]",
            Status::Done => "[x]",
            Status::Dropped => "[-]",
        }
    }
}

/// A step of a plan.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    /// What it is.
    pub step: String,
    /// Where it stands.
    pub status: Status,
    /// For a step done: how it was checked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked: Option<String>,
    /// For a step dropped: why.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

/// A session's plan: its steps in order, as the agent last wrote them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    /// The steps.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<Step>,
    /// How many times it was written.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub updates: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

impl Plan {
    /// Whether there is no plan.
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// The steps still to be done.
    pub fn open(&self) -> Vec<&Step> {
        self.steps.iter().filter(|s| s.status.is_open()).collect()
    }

    /// Writes the plan `input` gives ([`PLAN_UPDATE`]'s input): the plan as
    /// it now is, shown, or why it was refused, the plan left as it was.
    pub fn update(&mut self, input: &Value) -> Result<String, String> {
        let steps = parse(input)?;
        let kept: Vec<String> = steps.iter().map(|s| same(&s.step)).collect();
        if let Some(left) = self
            .steps
            .iter()
            .find(|old| old.status.is_open() && !kept.contains(&same(&old.step)))
        {
            return Err(format!(
                "\"{}\" was open, and is not in the new plan: keep it, or mark it dropped and say why",
                left.step
            ));
        }
        self.steps = steps;
        self.updates += 1;
        Ok(self.shown())
    }

    /// Each step in a line, numbered, with its mark: `[x]` done, `[>]`
    /// active, `[ ]` pending, `[-]` dropped.
    pub fn lines(&self) -> Vec<String> {
        self.steps
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let mut line = format!("{}. {} {}", i + 1, s.status.mark(), s.step);
                if let Some(checked) = &s.checked {
                    line.push_str(&format!(" (checked: {checked})"));
                }
                if let Some(why) = &s.why {
                    line.push_str(&format!(" (dropped: {why})"));
                }
                line
            })
            .collect()
    }

    /// The plan as the agent is shown it.
    pub fn shown(&self) -> String {
        format!("The plan, {}:\n{}", self.counted(), self.lines().join("\n"))
    }

    /// Where it stands, in a few words, for whoever watches.
    pub fn status(&self) -> String {
        match self.steps.iter().find(|s| s.status == Status::Active) {
            Some(active) => format!("{}; now: {}", self.counted(), active.step),
            None => self.counted(),
        }
    }

    fn counted(&self) -> String {
        let done = self
            .steps
            .iter()
            .filter(|s| s.status == Status::Done)
            .count();
        let dropped = self
            .steps
            .iter()
            .filter(|s| s.status == Status::Dropped)
            .count();
        let total = self.steps.len();
        match dropped {
            0 => format!("{done} of {total} steps done"),
            _ => format!("{done} of {total} steps done, {dropped} dropped"),
        }
    }
}

/// The steps `input` gives, checked: at most [`MAX_STEPS`], each saying
/// something, none twice, at most one active, each done with how it was
/// checked and each dropped with why.
fn parse(input: &Value) -> Result<Vec<Step>, String> {
    let Some(list) = input["steps"].as_array() else {
        return Err("give steps: the whole plan, a list of {step, status}".into());
    };
    if list.is_empty() {
        return Err("a plan has at least one step".into());
    }
    if list.len() > MAX_STEPS {
        return Err(format!(
            "a plan has at most {MAX_STEPS} steps: put the small ones together"
        ));
    }
    let mut steps: Vec<Step> = Vec::new();
    for (i, item) in list.iter().enumerate() {
        let n = i + 1;
        let text = item["step"].as_str().map(str::trim).unwrap_or_default();
        if text.is_empty() {
            return Err(format!("step {n} says nothing: give its step"));
        }
        if text.chars().count() > MAX_CHARS {
            return Err(format!(
                "step {n} is longer than {MAX_CHARS} characters: say it shorter"
            ));
        }
        let said = item["status"].as_str().unwrap_or("pending");
        let status = Status::parse(said).ok_or_else(|| {
            format!("step {n}'s status is pending, active, done or dropped, not {said:?}")
        })?;
        let note = |key: &str| -> Option<String> {
            item[key]
                .as_str()
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(|t| t.chars().take(MAX_CHARS).collect())
        };
        let (checked, why) = match status {
            Status::Done => {
                let Some(checked) = note("checked") else {
                    return Err(format!(
                        "step {n} is done only with how it was checked: give checked, what showed it (a test that passed, a command's output, a file read back)"
                    ));
                };
                (Some(checked), None)
            }
            Status::Dropped => {
                let Some(why) = note("why") else {
                    return Err(format!("step {n} is dropped only with why: give why"));
                };
                (None, Some(why))
            }
            Status::Pending | Status::Active => (None, None),
        };
        if steps.iter().any(|s| same(&s.step) == same(text)) {
            return Err(format!("step {n} is in the plan twice"));
        }
        steps.push(Step {
            step: text.to_string(),
            status,
            checked,
            why,
        });
    }
    if steps.iter().filter(|s| s.status == Status::Active).count() > 1 {
        return Err("at most one step is active at a time".into());
    }
    Ok(steps)
}

/// A step's words, compared: case, spacing and a closing stop aside.
fn same(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
        .trim_end_matches(['.', '!', ';', ':'])
        .to_string()
}

/// The tool the model is shown.
pub fn spec() -> ToolSpec {
    ToolSpec {
        name: PLAN_UPDATE.into(),
        description: "Write down the plan for a task of three or more steps, and where each \
                      stands: the whole plan each time, its steps in order, one active at a \
                      time. Update it as you go. A step is done only with how you checked it \
                      (the test that passed, the output that showed it); a step you will not \
                      do is dropped, with why. While steps are open, you are not done."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "steps": {
                    "type": "array",
                    "maxItems": MAX_STEPS,
                    "items": {
                        "type": "object",
                        "properties": {
                            "step": {"type": "string", "description": "What the step is, in a line"},
                            "status": {"type": "string", "enum": ["pending", "active", "done", "dropped"]},
                            "checked": {"type": "string", "description": "For a step done: what showed it is done"},
                            "why": {"type": "string", "description": "For a step dropped: why"}
                        },
                        "required": ["step", "status"]
                    }
                }
            },
            "required": ["steps"]
        }),
    }
}

/// What a model that says it is done is told when its plan has steps
/// still open.
pub fn reminder(open: &[&Step]) -> String {
    let steps: Vec<String> = open.iter().map(|s| format!("- {}", s.step)).collect();
    format!(
        "[Your plan still has steps open:\n{}\nDo them, or mark them dropped with {PLAN_UPDATE} \
         and say why, then finish. If you are waiting for the person, say what you need from \
         them.]",
        steps.join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(steps: Value) -> Result<String, String> {
        Plan::default().update(&json!({ "steps": steps }))
    }

    #[test]
    fn a_plan_is_written_whole_and_shown_with_its_marks() {
        let mut plan = Plan::default();
        let shown = plan
            .update(&json!({"steps": [
                {"step": "Reproduce the failure", "status": "completed", "checked": "the test failed as reported"},
                {"step": "Fix the parser", "status": "in_progress"},
                {"step": "Run the whole suite", "status": "pending"}
            ]}))
            .unwrap();
        assert_eq!(
            shown,
            "The plan, 1 of 3 steps done:\n1. [x] Reproduce the failure (checked: the test failed as reported)\n2. [>] Fix the parser\n3. [ ] Run the whole suite"
        );
        assert_eq!(plan.status(), "1 of 3 steps done; now: Fix the parser");
        assert_eq!(plan.open().len(), 2);
        assert_eq!(plan.updates, 1);
    }

    #[test]
    fn a_step_is_done_only_with_how_it_was_checked_and_dropped_only_with_why() {
        let done = plan(json!([{"step": "Fix it", "status": "done"}])).unwrap_err();
        assert!(done.contains("how it was checked"), "{done}");
        let dropped = plan(json!([{"step": "Fix it", "status": "dropped"}])).unwrap_err();
        assert!(dropped.contains("give why"), "{dropped}");
        assert!(
            plan(json!([{"step": "Fix it", "status": "dropped", "why": "not needed"}])).is_ok()
        );
        let two = plan(json!([
            {"step": "A", "status": "active"},
            {"step": "B", "status": "active"}
        ]))
        .unwrap_err();
        assert!(two.contains("one step is active"), "{two}");
        assert!(plan(json!([{"step": "A"}, {"step": "a."}]))
            .unwrap_err()
            .contains("twice"));
        assert!(plan(json!([{"step": "A", "status": "someday"}]))
            .unwrap_err()
            .contains("not \"someday\""));
        assert!(plan(json!([])).is_err());
        assert!(Plan::default().update(&json!({})).is_err());
        let many: Vec<Value> = (0..=MAX_STEPS)
            .map(|i| json!({"step": format!("step {i}")}))
            .collect();
        assert!(plan(Value::Array(many)).is_err());
    }

    #[test]
    fn an_open_step_is_not_left_out_without_a_word() {
        let mut plan = Plan::default();
        plan.update(&json!({"steps": [
            {"step": "Fix the parser", "status": "active"},
            {"step": "Run the suite"}
        ]}))
        .unwrap();
        let refused = plan
            .update(&json!({"steps": [{"step": "Fix the parser", "status": "done", "checked": "its test passes"}]}))
            .unwrap_err();
        assert!(refused.contains("\"Run the suite\" was open"), "{refused}");
        // Refused, the plan is as it was.
        assert_eq!(plan.open().len(), 2);
        // Kept with other spacing and case, or dropped with why, it is fine.
        plan.update(&json!({"steps": [
            {"step": "fix  the parser.", "status": "done", "checked": "its test passes"},
            {"step": "Run the suite", "status": "dropped", "why": "the person asked for the parser only"}
        ]}))
        .unwrap();
        assert!(plan.open().is_empty());
        // Steps no longer open may be left out.
        plan.update(&json!({"steps": [{"step": "Write the note"}]}))
            .unwrap();
        assert_eq!(plan.steps.len(), 1);
    }

    #[test]
    fn the_reminder_names_the_open_steps() {
        let mut plan = Plan::default();
        plan.update(&json!({"steps": [
            {"step": "A", "status": "done", "checked": "seen"},
            {"step": "B"}
        ]}))
        .unwrap();
        let told = reminder(&plan.open());
        assert!(told.contains("- B\n"), "{told}");
        assert!(!told.contains("- A"), "{told}");
    }
}
