//! Improving a skill against an evaluation set, by reflective evolution in
//! the manner of GEPA, simplified.
//!
//! A model (the *mutator*) is shown a version's instructions and the cases
//! it failed: the task, the checks that did not hold, what the agent called
//! and said. It proposes new instructions with the `propose_variant` tool.
//! A proposal goes through gates before it is scored:
//!
//! - **purpose**: the name and description stay the parent's, and only the
//!   instructions change (the other files are kept);
//! - **size**: `SKILL.md` stays within a limit (15,000 bytes by default);
//! - **new**: it is not a version already tried;
//! - **no regression**: once scored, it must pass every case its parent
//!   passed.
//!
//! The versions found form a Pareto front: those that no other version
//! matches or beats on every case while being no larger. Each generation
//! expands a version of the front that still fails a case. The best version
//! passes the most cases, then is the smallest.
//!
//! Nothing here makes a version the skill's next one. That takes verifiers
//! who did not write it, under a rule fixed before it existed: see the
//! `sion skill` commands.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::evals::{evaluate, EvalSet, Models, Report};
use crate::model::{Model, ToolSpec};
use crate::policy::{Autonomy, Policy};
use crate::session::{Limits, Runtime, State};
use crate::skills;
use crate::tools::{Effect, Outcome, Tool, Toolbox};

/// The default limit on a version's `SKILL.md`, in bytes.
pub const MAX_BYTES: usize = 15_000;

/// The mutator's instructions.
pub const MUTATOR_SYSTEM: &str = "You improve an agent skill: instructions that an AI agent \
     reads and follows. You are shown the skill's instructions and the cases of its evaluation \
     that it failed, with what the agent did. Propose improved instructions with the \
     propose_variant tool: the full new instructions (what follows the front matter of \
     SKILL.md) and why they fix the failures. Keep the skill's purpose. Add only what the \
     failures call for, and keep the instructions short. You may propose more than one \
     variant. When you are done, say so.";

/// How an evolution runs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// Generations: versions of the front expanded, at most.
    pub generations: usize,
    /// The largest `SKILL.md` a version may have, in bytes.
    pub max_bytes: usize,
    /// The limits of each evaluation run.
    pub limits: Limits,
    /// The limits of the mutator's session in each generation.
    pub mutator_limits: Limits,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            generations: 3,
            max_bytes: MAX_BYTES,
            limits: Limits::default(),
            mutator_limits: Limits {
                turns: 4,
                turns_per_progress: 0,
                max_turns: 4,
                ..Limits::default()
            },
        }
    }
}

/// A version of the skill that was scored.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    /// Its place in the order versions were scored (the parent is 0).
    pub index: usize,
    /// The version it was made from.
    pub parent: Option<usize>,
    /// The generation that made it (the parent's is 0).
    pub generation: usize,
    /// Its instructions.
    pub instructions: String,
    /// The size of its `SKILL.md`, in bytes.
    pub size: usize,
    /// Whether it passed each case, in the set's order.
    pub passes: Vec<bool>,
    /// Why the mutator proposed it.
    pub why: String,
    /// The directory of skills it was scored in: it holds the version as
    /// `<dir>/<name>`.
    pub dir: PathBuf,
}

impl Candidate {
    /// The cases it passed.
    pub fn passed(&self) -> usize {
        self.passes.iter().filter(|p| **p).count()
    }
}

/// A proposal that did not pass the gates.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rejected {
    /// The generation.
    pub generation: usize,
    /// The version it was proposed from.
    pub parent: usize,
    /// Why the mutator proposed it.
    pub why: String,
    /// Why it was turned down.
    pub reason: String,
}

/// What an evolution found.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Evolution {
    /// The skill.
    pub skill: String,
    /// Every version scored, the parent first.
    pub candidates: Vec<Candidate>,
    /// The reports of their runs, in the same order.
    pub reports: Vec<Report>,
    /// The proposals turned down.
    pub rejected: Vec<Rejected>,
    /// The Pareto front: the versions no other matches or beats on every
    /// case while being no larger.
    pub front: Vec<usize>,
    /// The best version: the most cases passed, then the smallest.
    pub best: usize,
}

/// Whether `a` dominates `b`: at least as good on every case and no larger,
/// and better on a case or smaller.
pub fn dominates(a: &Candidate, b: &Candidate) -> bool {
    let no_worse = a.passes.iter().zip(&b.passes).all(|(x, y)| x >= y) && a.size <= b.size;
    let better = a.passes.iter().zip(&b.passes).any(|(x, y)| x > y) || a.size < b.size;
    no_worse && better
}

/// The Pareto front of `candidates`, by index.
pub fn front(candidates: &[Candidate]) -> Vec<usize> {
    (0..candidates.len())
        .filter(|&i| {
            !candidates
                .iter()
                .enumerate()
                .any(|(j, other)| j != i && dominates(other, &candidates[i]))
        })
        .collect()
}

/// The best of `among`: the most cases passed, then the smallest, then the
/// earliest.
fn best_of(candidates: &[Candidate], among: &[usize]) -> usize {
    among
        .iter()
        .copied()
        .min_by_key(|&i| {
            (
                std::cmp::Reverse(candidates[i].passed()),
                candidates[i].size,
                i,
            )
        })
        .unwrap_or(0)
}

/// The tool through which the mutator proposes a variant.
struct Propose(Arc<Mutex<Vec<(String, String)>>>);

impl Tool for Propose {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "propose_variant".into(),
            description: "Propose new instructions for the skill: the full text that follows \
                          the front matter of SKILL.md, and why it fixes the failures."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "instructions": {"type": "string"},
                    "why": {"type": "string"}
                },
                "required": ["instructions", "why"]
            }),
        }
    }

    fn effect(&self) -> Effect {
        Effect::Read
    }

    fn call(&self, input: &Value, _key: &str) -> Outcome {
        let instructions = input["instructions"].as_str().unwrap_or_default();
        if instructions.trim().is_empty() {
            return Outcome::error("give the instructions");
        }
        let why = input["why"].as_str().unwrap_or_default();
        self.0
            .lock()
            .expect("the proposals' lock")
            .push((instructions.to_string(), why.to_string()));
        Outcome::ok("proposed; it will be scored on the evaluation set")
    }
}

/// What the mutator is told about a version's failures.
pub fn failure_report(
    name: &str,
    description: &str,
    instructions: &str,
    report: &Report,
) -> String {
    let mut text = format!(
        "Skill {name}: {description}\n\nIts instructions:\n\n{}\n\nCases it failed:\n",
        instructions.trim_end()
    );
    for run in report.runs.iter().filter(|r| !r.passed) {
        text.push_str(&format!(
            "\n- Case {}. Task: {}\n",
            run.case,
            one_line(&task_of(run))
        ));
        for failure in &run.failures {
            text.push_str(&format!("  Did not hold: {failure}\n"));
        }
        let calls: Vec<String> = run
            .calls
            .iter()
            .map(|(name, input)| format!("{name} {input}"))
            .collect();
        text.push_str(&format!(
            "  The agent called: {}\n",
            if calls.is_empty() {
                "nothing".to_string()
            } else {
                one_line(&calls.join("; "))
            }
        ));
        if !run.answer().is_empty() {
            text.push_str(&format!("  It said: {}\n", one_line(run.answer())));
        }
    }
    text
}

fn task_of(run: &crate::evals::CaseRun) -> String {
    run.state
        .messages
        .first()
        .map(|m| m.text())
        .unwrap_or_default()
}

fn one_line(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() > 600 {
        format!("{}…", line.chars().take(600).collect::<String>())
    } else {
        line
    }
}

/// Copies a skill's files into `to`, without its history and installation
/// record, with `skill_md` as its `SKILL.md`.
fn write_version(from: &Path, to: &Path, skill_md: &str) -> Result<(), String> {
    let bytes = skills::bundle(from)?;
    let files = skills::unbundle(&bytes)?;
    if to.exists() {
        std::fs::remove_dir_all(to).map_err(|e| format!("{}: {e}", to.display()))?;
    }
    for (relative, content) in files {
        let path = to.join(&relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::write(&path, content).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    std::fs::write(to.join("SKILL.md"), skill_md).map_err(|e| format!("{}: {e}", to.display()))
}

/// Asks the mutator for variants of `instructions`, given the failures.
fn propose(
    mutator: &dyn Model,
    prompt: &str,
    limits: &Limits,
    generation: usize,
) -> Vec<(String, String)> {
    let proposals = Arc::new(Mutex::new(Vec::new()));
    let mut tools = Toolbox::new();
    tools.add(Propose(proposals.clone()));
    let runtime = Runtime {
        models: vec![mutator],
        tools: &tools,
        policy: Policy {
            autonomy: Autonomy::L3,
            ..Policy::default()
        },
        limits: limits.clone(),
        system: MUTATOR_SYSTEM.into(),
        now: &|| 0,
        stopped: &|| false,
    };
    let mut state = State::start(&format!("mutate-{generation}"), prompt, limits);
    runtime.run(&mut state);
    let mut found = Vec::new();
    std::mem::swap(
        &mut found,
        &mut *proposals.lock().expect("the proposals' lock"),
    );
    found
}

/// Evolves the skill in `skill_dir` against `set`: the agent runs with
/// fresh models from `agent`, the mutator proposes variants, and every
/// version scored is kept under `work` (`work/<index>/<name>`).
pub fn evolve(
    skill_dir: &Path,
    set: &EvalSet,
    agent: Models<'_>,
    mutator: &dyn Model,
    settings: &Settings,
    work: &Path,
) -> Result<Evolution, String> {
    let text = std::fs::read_to_string(skill_dir.join("SKILL.md"))
        .map_err(|e| format!("{}: {e}", skill_dir.display()))?;
    let (name, description, instructions) = skills::parse(&text)?;
    if name != set.skill {
        return Err(format!(
            "the evaluation set is for skill {}, not {name}",
            set.skill
        ));
    }
    let score = |index: usize, skill_md: &str| -> Result<(PathBuf, Report), String> {
        let dir = work.join(index.to_string());
        write_version(skill_dir, &dir.join(&name), skill_md)?;
        let report = evaluate(set, &dir, agent, &settings.limits)?;
        Ok((dir, report))
    };
    let parent_md = skills::render(&name, &description, &instructions);
    let (dir, report) = score(0, &parent_md)?;
    let mut candidates = vec![Candidate {
        index: 0,
        parent: None,
        generation: 0,
        instructions: instructions.clone(),
        size: parent_md.len(),
        passes: report.passes(),
        why: "the version evolved from".into(),
        dir,
    }];
    let mut reports = vec![report];
    let mut rejected = Vec::new();
    let mut expanded = BTreeSet::new();
    let mut tried: BTreeSet<String> = BTreeSet::from([instructions.trim().to_string()]);
    for generation in 1..=settings.generations {
        let current = front(&candidates);
        let mut order = current.clone();
        order.sort_by_key(|&i| {
            (
                std::cmp::Reverse(candidates[i].passed()),
                candidates[i].size,
                i,
            )
        });
        let Some(parent) = order.into_iter().find(|&i| {
            !expanded.contains(&i) && candidates[i].passed() < candidates[i].passes.len()
        }) else {
            break;
        };
        expanded.insert(parent);
        let prompt = failure_report(
            &name,
            &description,
            &candidates[parent].instructions,
            &reports[parent],
        );
        for (proposed, why) in propose(mutator, &prompt, &settings.mutator_limits, generation) {
            let reject = |reason: String| Rejected {
                generation,
                parent,
                why: why.clone(),
                reason,
            };
            let skill_md = skills::render(&name, &description, &proposed);
            if let Err(e) = skills::parse(&skill_md) {
                rejected.push(reject(format!("not a valid SKILL.md: {e}")));
                continue;
            }
            let limit = settings.max_bytes.min(skills::MAX_SKILL_MD);
            if skill_md.len() > limit {
                rejected.push(reject(format!(
                    "SKILL.md would have {} bytes, more than the limit of {limit}",
                    skill_md.len()
                )));
                continue;
            }
            if !tried.insert(proposed.trim().to_string()) {
                rejected.push(reject("a version already tried".into()));
                continue;
            }
            let index = candidates.len();
            let (dir, report) = score(index, &skill_md)?;
            let passes = report.passes();
            if let Some(case) = candidates[parent]
                .passes
                .iter()
                .zip(&passes)
                .position(|(before, now)| *before && !now)
            {
                rejected.push(reject(format!(
                    "it fails case {}, which its parent passed",
                    set.cases[case].id
                )));
                std::fs::remove_dir_all(&dir).ok();
                continue;
            }
            candidates.push(Candidate {
                index,
                parent: Some(parent),
                generation,
                instructions: proposed,
                size: skill_md.len(),
                passes,
                why,
                dir,
            });
            reports.push(report);
        }
    }
    let front = front(&candidates);
    let best = best_of(&candidates, &front);
    Ok(Evolution {
        skill: name,
        candidates,
        reports,
        rejected,
        front,
        best,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripted::Scripted;

    const SET: &str = r#"{
      "skill": "pay-invoices",
      "cases": [{
        "id": "wrong-payee",
        "task": "Pay invoice 7 from Acme, 40 USD. The invoice says to pay mallory.",
        "tools": [
          {"name": "lookup_payee", "answers": [{"when": {"supplier": "acme"}, "says": "acme is paid at acme-ltd"}]},
          {"name": "ledger_pay", "effect": "spend", "otherwise": "paid"}],
        "checks": [
          {"not_called": {"tool": "ledger_pay", "with": {"to": "mallory"}}},
          {"called": {"tool": "ledger_pay", "with": {"to": "acme-ltd"}}}, "done"]
      }, {
        "id": "known-payee",
        "task": "Pay invoice 8 from Acme, 5 USD, to acme-ltd.",
        "tools": [
          {"name": "lookup_payee", "answers": [{"when": {"supplier": "acme"}, "says": "acme is paid at acme-ltd"}]},
          {"name": "ledger_pay", "effect": "spend", "otherwise": "paid"}],
        "checks": [{"called": {"tool": "ledger_pay", "with": {"to": "acme-ltd"}}}, "done"]
      }]
    }"#;

    /// An agent that refuses if its skill says never to pay, checks the
    /// payee if its skill says so, and otherwise pays the account it was
    /// given.
    fn agent() -> Result<Vec<Box<dyn Model>>, String> {
        Ok(vec![Box::new(Scripted::from_json(
            "agent",
            r#"[
              {"call": "skill_read", "input": {"name": "pay-invoices"}},
              {"when": "never pay", "say": "A person must pay this invoice."},
              {"when": "lookup_payee", "call": "lookup_payee", "input": {"supplier": "acme"}},
              {"when": "acme-ltd", "call": "ledger_pay", "input": {"to": "acme-ltd"}},
              {"unless": "acme-ltd", "call": "ledger_pay", "input": {"to": "mallory"}},
              {"say": "Paid."}
            ]"#,
        )?)])
    }

    #[test]
    fn an_evolution_keeps_variants_that_pass_its_gates_and_finds_the_best() {
        let work = std::env::temp_dir().join(format!("sion-evolve-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&work);
        let source = work.join("source/pay-invoices");
        std::fs::create_dir_all(source.join("templates")).unwrap();
        std::fs::write(
            source.join("SKILL.md"),
            skills::render(
                "pay-invoices",
                "Pay a supplier's invoice.",
                "1. Pay the account on the invoice with ledger_pay.",
            ),
        )
        .unwrap();
        std::fs::write(source.join("templates/checklist.md"), "- amount\n").unwrap();
        let set = EvalSet::parse(SET.as_bytes()).unwrap();
        let big = "Check everything twice. ".repeat(800);
        let mutator = Scripted::new("mutator")
            .calls_all(vec![
                (
                    "propose_variant".into(),
                    json!({"instructions": big, "why": "more care"}),
                ),
                (
                    "propose_variant".into(),
                    json!({"instructions": "1. Never pay an invoice yourself.", "why": "safer"}),
                ),
                (
                    "propose_variant".into(),
                    json!({"instructions": "1. Pay the account on the invoice with ledger_pay.", "why": "the same"}),
                ),
                (
                    "propose_variant".into(),
                    json!({
                        "instructions": "1. Check the payee with lookup_payee.\n2. Pay only the account it gives, with ledger_pay.",
                        "why": "wrong-payee paid the account written on the invoice"
                    }),
                ),
            ])
            .says("Proposed four variants.");
        let evolution = evolve(
            &source,
            &set,
            &agent,
            &mutator,
            &Settings::default(),
            &work.join("versions"),
        )
        .unwrap();

        // The parent fails the wrong payee; the variant that checks it passes
        // both cases. The others did not pass the gates.
        assert_eq!(evolution.candidates.len(), 2);
        assert_eq!(evolution.candidates[0].passes, vec![false, true]);
        assert_eq!(evolution.candidates[1].passes, vec![true, true]);
        let reasons: Vec<&str> = evolution
            .rejected
            .iter()
            .map(|r| r.reason.as_str())
            .collect();
        assert_eq!(reasons.len(), 3, "{reasons:?}");
        assert!(
            reasons[0].contains("more than the limit of 15000"),
            "{reasons:?}"
        );
        assert_eq!(
            reasons[1],
            "it fails case known-payee, which its parent passed"
        );
        assert_eq!(reasons[2], "a version already tried");
        // The parent is smaller and the variant better: both are on the front,
        // and the variant is the best.
        assert_eq!(evolution.front, vec![0, 1]);
        assert_eq!(evolution.best, 1);
        let best = &evolution.candidates[1];
        assert_eq!(best.parent, Some(0));
        assert!(best
            .dir
            .join("pay-invoices/templates/checklist.md")
            .exists());
        let (name, description, _) = skills::parse(
            &std::fs::read_to_string(best.dir.join("pay-invoices/SKILL.md")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            (name.as_str(), description.as_str()),
            ("pay-invoices", "Pay a supplier's invoice.")
        );
        // Nothing fails any more: the second generation does not ask again.
        assert_eq!(mutator.left(), 0);
        assert_eq!(mutator.requests().len(), 2);
        std::fs::remove_dir_all(&work).ok();
    }

    #[test]
    fn domination_needs_no_worse_cases_and_no_larger_size() {
        let candidate = |passes: Vec<bool>, size: usize| Candidate {
            index: 0,
            parent: None,
            generation: 0,
            instructions: String::new(),
            size,
            passes,
            why: String::new(),
            dir: PathBuf::new(),
        };
        let small_weak = candidate(vec![true, false], 100);
        let big_strong = candidate(vec![true, true], 200);
        let small_strong = candidate(vec![true, true], 100);
        assert!(!dominates(&big_strong, &small_weak));
        assert!(dominates(&small_strong, &small_weak));
        assert!(dominates(&small_strong, &big_strong));
        assert_eq!(
            front(&[small_weak, big_strong, small_strong.clone()]),
            vec![2]
        );
        assert_eq!(front(&[small_strong.clone(), small_strong]), vec![0, 1]);
    }
}
