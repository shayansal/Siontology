//! Practice tasks the agent's model writes for it (`sion agent improve
//! practice`): tasks like the seal's tasks to choose on, never its sealed
//! ones, to choose changes on beside them (`run --practice`).
//!
//! The writer is shown what the operator tells the agent and tasks to
//! choose on, those the agent fails first. Each task it writes is kept only
//! when its own solution passes it and each of its failures fails it
//! ([`sion_agent::practice::verify`]), when it is not a sealed task or a
//! task to choose on over again, and when the agent as it is does not pass
//! it every time: a task it always passes shows no change anything. A
//! canary's code word in one means the sealed tasks have reached the
//! writer, and nothing is kept.
//!
//! Practice tasks only ever join the tasks changes are chosen on. A change
//! is still confirmed on the sealed tasks alone, so a practice task that
//! misleads costs a change that was tried, not one that was kept.

use std::path::{Path, PathBuf};

use sion_agent::compare::TaskSet;
use sion_agent::evals::Case;
use sion_agent::model::Model;
use sion_core::hash::Hash32;

use crate::error::CliError;
use crate::improve_loop::{clip, score, Variant};
use crate::profiles::Profile;

/// The most practice tasks asked for at once.
pub const MAX_PRACTICE: usize = 64;

/// What the writer of practice tasks is told it is.
pub const PRACTICE_SYSTEM: &str = "You write practice tasks for an AI agent: tasks like those it \
is given at work, to try changes to it on. You write each with write_task, with a solution and \
failures that show its checks mean what they say.";

/// What a written task is to the sealed tasks, when it is anything.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Against {
    /// It holds the code word of this canary: the sealed tasks reached its
    /// writer.
    Canary(String),
    /// It asks what this sealed task asks, word for word.
    Copy(String),
}

/// What `case` is to the `sealed` cases: a canary's code word in what it
/// says, or a sealed task's request word for word.
pub fn against_sealed(case: &Case, sealed: &[Case]) -> Option<Against> {
    let texts: Vec<String> = sion_agent::guard::case_texts(case)
        .into_iter()
        .map(|t| t.to_lowercase())
        .collect();
    for seen in sealed {
        match sion_agent::guard::canary_word(seen) {
            Some(word) => {
                let word = word.to_lowercase();
                if texts.iter().any(|t| t.contains(&word)) {
                    return Some(Against::Canary(seen.id.clone()));
                }
            }
            None if sion_agent::practice::same_task(&case.task, &seen.task) => {
                return Some(Against::Copy(seen.id.clone()));
            }
            None => {}
        }
    }
    None
}

/// The writer's prompt: what the operator tells the agent, then up to a
/// dozen of `train`'s tasks, those `passed` says the agent fails first.
pub fn practice_prompt(
    operator: Option<&str>,
    train: &TaskSet,
    passed: &[bool],
    count: usize,
) -> String {
    let mut text = String::new();
    if let Some(words) = operator.filter(|w| !w.trim().is_empty()) {
        text.push_str(&format!(
            "What the agent's operator tells it:\n{}\n\n",
            clip(words, 4_000)
        ));
    }
    text.push_str(
        "Tasks like those it is given, with their tools and what a run of each is checked for:\n",
    );
    let mut order: Vec<usize> = (0..train.cases.len()).collect();
    order.sort_by_key(|&i| passed.get(i).copied().unwrap_or(true));
    for &i in order.iter().take(12) {
        let shown = sion_agent::practice::shown(&train.cases[i]);
        if passed.get(i) == Some(&false) {
            text.push_str(shown.trim_end());
            text.push_str("\n  (The agent fails this one now.)\n");
        } else {
            text.push_str(&shown);
        }
    }
    text.push_str(&format!(
        "\nWrite {count} new practice tasks with write_task: each its own request of these kinds, \
         with names, amounts and wording of its own, as hard as these or harder, above all where \
         the agent fails them. Do not copy a task. Give each the stand-ins it needs, checks a right \
         run passes and a wrong one fails, a solution and at least one failure."
    ));
    text
}

/// Practice tasks written for a profile.
#[derive(Clone, Debug)]
pub struct Practiced {
    /// The tasks kept, if any were.
    pub set: Option<TaskSet>,
    /// What happened, a line each.
    pub lines: Vec<String>,
    /// The tokens used, the writer's and the agent's.
    pub tokens: u64,
}

/// Has `writer` write up to `count` practice tasks for `profile`'s agent,
/// shown `train` and never `sealed`; runs the agent as it is `runs` times
/// on each that its plays bear out, within `budget_tokens` a run, and
/// keeps those it does not pass every time.
#[allow(clippy::too_many_arguments)]
pub fn write_practice(
    profile: &Profile,
    writer: &dyn Model,
    writer_name: &str,
    train: &TaskSet,
    sealed: &TaskSet,
    count: usize,
    runs: u32,
    budget_tokens: u64,
) -> Result<Practiced, CliError> {
    if !(1..=MAX_PRACTICE).contains(&count) {
        return Err(CliError::Usage(format!(
            "ask for 1 to {MAX_PRACTICE} practice tasks"
        )));
    }
    let variant = Variant::of(profile);
    let train_digest = digest_of(train)?;
    // One run of each task to choose on, to show the writer what the agent
    // fails.
    let examples = score(profile, &variant, train, train_digest, 1, budget_tokens)?;
    let mut tokens = examples.outcome.tokens;
    let prompt = practice_prompt(
        profile.instructions.as_deref(),
        train,
        &examples.cases,
        count,
    );
    let writing = sion_agent::practice::write(writer, PRACTICE_SYSTEM, &prompt, count);
    tokens += writing.tokens;
    let mut lines = Vec::new();
    let mut cases: Vec<Case> = Vec::new();
    let (mut copies, mut repeats) = (0, 0);
    for (at, written) in writing.written.iter().enumerate() {
        let case = written.case(&format!("practice-{at}"));
        match against_sealed(&case, &sealed.cases) {
            Some(Against::Canary(canary)) => {
                return Err(CliError::Usage(format!(
                    "a task the writer wrote holds the code word of {canary}: the sealed tasks have reached it; seal fresh tasks, and keep them from {writer_name}"
                )));
            }
            Some(Against::Copy(_)) => {
                copies += 1;
                continue;
            }
            None => {}
        }
        if train
            .cases
            .iter()
            .chain(&cases)
            .any(|c| sion_agent::practice::same_task(&c.task, &case.task))
        {
            repeats += 1;
            continue;
        }
        cases.push(case);
    }
    lines.push(format!(
        "{writer_name} wrote {} tasks, {} borne out by their own solutions and failures",
        writing.written.len() + writing.refused.len(),
        writing.written.len()
    ));
    if !writing.refused.is_empty() {
        lines.push(format!(
            "  refused, as it was told: {}",
            clip(&writing.refused.join("; "), 600)
        ));
    }
    if copies > 0 {
        lines.push(format!(
            "  left out: {copies} the same as a sealed task (it was never shown them)"
        ));
    }
    if repeats > 0 {
        lines.push(format!(
            "  left out: {repeats} the same as a task to choose on, or as another it wrote"
        ));
    }
    if cases.is_empty() {
        return Ok(Practiced {
            set: None,
            lines,
            tokens,
        });
    }
    let candidates = TaskSet {
        name: set_name(&profile.name),
        about: format!(
            "practice tasks {writer_name} wrote for {}, borne out by their own solutions and failures, which the agent as it was did not pass every time",
            profile.name
        ),
        system: train.system.clone(),
        cases,
    };
    let scored = score(
        profile,
        &variant,
        &candidates,
        digest_of(&candidates)?,
        runs,
        budget_tokens,
    )?;
    tokens += scored.outcome.tokens;
    let mut kept = Vec::new();
    let mut passed_always = 0;
    for case in &candidates.cases {
        let trials: Vec<bool> = scored
            .result
            .trials
            .iter()
            .filter(|t| t.case == case.id)
            .map(|t| t.passed)
            .collect();
        if !trials.is_empty() && trials.iter().all(|p| *p) {
            passed_always += 1;
        } else {
            kept.push(case.clone());
        }
    }
    if passed_always > 0 {
        lines.push(format!(
            "  left out: {passed_always} the agent as it is passes in every run of {runs}"
        ));
    }
    let set = if kept.is_empty() {
        None
    } else {
        Some(TaskSet {
            cases: kept,
            ..candidates
        })
    };
    Ok(Practiced { set, lines, tokens })
}

/// `n` practice tasks, in words.
pub fn counted(n: usize) -> String {
    if n == 1 {
        "1 practice task".into()
    } else {
        format!("{n} practice tasks")
    }
}

/// A set's digest: of its bytes as written.
pub fn digest_of(set: &TaskSet) -> Result<Hash32, CliError> {
    serde_json::to_vec_pretty(set)
        .map(|bytes| sion_agent::evals::digest(&bytes))
        .map_err(|e| CliError::Usage(format!("the tasks could not be written: {e}")))
}

fn set_name(profile: &str) -> String {
    let mut name: String = profile
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    name.truncate(40);
    let name = name.trim_matches('-');
    if name.is_empty() {
        "agent-practice".into()
    } else {
        format!("{name}-practice")
    }
}

/// Where `profile`'s practice tasks of digest `digest` are written unless
/// told otherwise.
pub fn practice_path(profile: &str, digest: &Hash32) -> PathBuf {
    crate::model_routes::dir().join(format!(
        "{profile}-practice-{}.json",
        &digest.to_hex()[..16]
    ))
}

/// `train` with the practice tasks of the file at `path` after its own,
/// each renamed `practice-<n>`: the set, the practice file's digest, and a
/// line on what was taken. A practice task that holds a canary's code word
/// stops it; one that is a sealed task or a task to choose on over again is
/// left out.
pub fn joined(
    train: &TaskSet,
    path: &Path,
    sealed: &TaskSet,
) -> Result<(TaskSet, Hash32, String), CliError> {
    let bytes =
        std::fs::read(path).map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?;
    let practice =
        TaskSet::parse(&bytes).map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?;
    let digest = sion_agent::evals::digest(&bytes);
    let mut set = train.clone();
    let (mut taken, mut left_out) = (0, 0);
    for (at, case) in practice.cases.iter().enumerate() {
        match against_sealed(case, &sealed.cases) {
            Some(Against::Canary(canary)) => {
                return Err(CliError::Usage(format!(
                    "practice task {} holds the code word of {canary}: the sealed tasks have reached its writer; seal fresh tasks",
                    case.id
                )));
            }
            Some(Against::Copy(_)) => {
                left_out += 1;
                continue;
            }
            None => {}
        }
        if set
            .cases
            .iter()
            .any(|c| sion_agent::practice::same_task(&c.task, &case.task))
            || set.cases.len() >= sion_agent::evals::MAX_CASES
        {
            left_out += 1;
            continue;
        }
        let mut case = case.clone();
        case.id = format!("practice-{at}");
        set.cases.push(case);
        taken += 1;
    }
    sion_agent::evals::check_cases(&set.cases).map_err(CliError::Usage)?;
    let line = format!(
        "with {} from {} among the tasks to choose on{}",
        counted(taken),
        path.display(),
        if left_out > 0 {
            format!(" ({left_out} left out: the same as a sealed task or a task to choose on, or past the {} a set holds)", sion_agent::evals::MAX_CASES)
        } else {
            String::new()
        }
    );
    Ok((set, digest, line))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sion_agent::evals::Check;

    fn case(id: &str, task: &str) -> Case {
        Case {
            id: id.into(),
            task: task.into(),
            tools: Vec::new(),
            checks: vec![Check::Done],
            person: None,
        }
    }

    #[test]
    fn a_written_task_is_held_to_the_sealed_ones_by_copy_and_by_canary() {
        let sealed = vec![
            case("work-1", "Pay invoice 9 from Acme, 5 USD, to acme-ltd."),
            sion_agent::guard::canary(0, "PLOVER-17"),
        ];
        // Like a sealed task, as tasks of one kind are, but not it: kept.
        assert_eq!(
            against_sealed(
                &case("p", "Pay invoice 10 from Acme, 5 USD, to acme-ltd."),
                &sealed
            ),
            None
        );
        assert_eq!(
            against_sealed(
                &case("p", "pay invoice 9 from acme 5 usd to ACME LTD"),
                &sealed
            ),
            Some(Against::Copy("work-1".into()))
        );
        let mut says = case("p", "Answer.");
        says.checks.push(Check::Says("plover-17".into()));
        assert_eq!(
            against_sealed(&says, &sealed),
            Some(Against::Canary("canary-0".into()))
        );
        let prompt = practice_prompt(
            Some("You pay invoices."),
            &TaskSet {
                name: "t".into(),
                about: String::new(),
                system: None,
                cases: vec![case("a", "Easy one."), case("b", "Hard one.")],
            },
            &[true, false],
            3,
        );
        // The task it fails is shown first, and marked.
        assert!(prompt.find("Hard one.").unwrap() < prompt.find("Easy one.").unwrap());
        assert!(prompt.contains("(The agent fails this one now.)"));
        assert!(prompt.contains("Write 3 new practice tasks"));
    }
}
