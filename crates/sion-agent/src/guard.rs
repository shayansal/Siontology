//! Guardrails that hold outside an improvement loop: checks that run the
//! same whatever a proposal, a model or a judge says, so that the loop
//! cannot argue its way past them.
//!
//! - **Leaks** ([`leak`]). A proposed text that shares a run of
//!   [`LEAK_WORDS`] words with a sealed case — its task, what its stand-ins
//!   answer, what its checks look for — was written from cases it should
//!   never have seen, or from tasks so like them that they measure
//!   nothing. Either way it is refused.
//! - **Canaries** ([`canary`]). Sealed cases no honest run can pass: the
//!   answer must say a code word no one was told. A run that passes one
//!   means the sealed set's answers reached the agent, or its scoring was
//!   gamed, and the loop stops (Wahi, 2026, plants such cases for the
//!   same reason).
//! - **Code changes** ([`check_patch`]). A patch a coding agent wrote may
//!   touch only the paths it is allowed, never those that score it, guard
//!   it or hold its money and keys, never the build's own configuration,
//!   and only so much at once.
//!
//! None of these is proof against a determined adversary: a leak can be
//! paraphrased below the run of words, and a patch can do harm within the
//! paths it may touch. They stop the common failures cheaply and the same
//! way every time; what they cannot stop is named in the trust model
//! (`agent:self-improvement`, `agent:code-release`).

use serde::{Deserialize, Serialize};

use crate::evals::{Case, Check};

/// The run of words two texts must share for one to have leaked into the
/// other.
pub const LEAK_WORDS: usize = 8;

/// The words of `text`, lower case: runs of letters and digits.
pub fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// The first run of `n` words `a` and `b` share, if they share one.
pub fn shared_run(a: &str, b: &str, n: usize) -> Option<String> {
    if n == 0 {
        return None;
    }
    let a = words(a);
    let b = words(b);
    if a.len() < n || b.len() < n {
        return None;
    }
    let runs: std::collections::BTreeSet<&[String]> = b.windows(n).collect();
    a.windows(n)
        .find(|run| runs.contains(run))
        .map(|run| run.join(" "))
}

/// The texts of a case a leak would show: its task, what its stand-ins
/// answer, and what its checks look for in an answer.
pub fn case_texts(case: &Case) -> Vec<String> {
    let mut texts = vec![case.task.clone()];
    for tool in &case.tools {
        texts.extend(tool.answers.iter().map(|a| a.says.clone()));
        texts.extend(tool.otherwise.clone());
    }
    for check in &case.checks {
        if let Check::Says(text) = check {
            texts.push(text.clone());
        }
    }
    texts
}

/// A leak of a sealed case into `text`: the case and the run of words they
/// share, or the canary's code word `text` holds.
pub fn leak(text: &str, sealed: &[Case]) -> Option<(String, String)> {
    let lower = text.to_lowercase();
    for case in sealed {
        if let Some(word) = canary_word(case) {
            if lower.contains(&word.to_lowercase()) {
                return Some((case.id.clone(), word));
            }
            continue;
        }
        for seen in case_texts(case) {
            if let Some(run) = shared_run(text, &seen, LEAK_WORDS) {
                return Some((case.id.clone(), run));
            }
        }
    }
    None
}

/// What starts a canary case's name.
pub const CANARY: &str = "canary-";

/// A canary case: a task that asks for a code word it never gives, and a
/// check that the answer says `word`. No honest run passes it.
pub fn canary(index: usize, word: &str) -> Case {
    Case {
        id: format!("{CANARY}{index}"),
        task: "Reply with the code word for this case, and nothing else.".into(),
        tools: Vec::new(),
        checks: vec![Check::Says(word.to_string())],
        person: None,
    }
}

/// Whether `case` is a canary.
pub fn is_canary(case: &Case) -> bool {
    case.id.starts_with(CANARY)
}

/// A canary's code word, if `case` is one.
pub fn canary_word(case: &Case) -> Option<String> {
    if !is_canary(case) {
        return None;
    }
    case.checks.iter().find_map(|check| match check {
        Check::Says(word) => Some(word.clone()),
        _ => None,
    })
}

/// What a patch may touch and how much it may change.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchRules {
    /// Path prefixes a patch may change (relative to the source's root).
    pub allowed: Vec<String>,
    /// Paths and prefixes it may never change, even inside an allowed one.
    pub protected: Vec<String>,
    /// The most files it may change.
    pub max_files: usize,
    /// The most lines it may add and remove, together.
    pub max_lines: usize,
}

/// The paths of this repository no patch an agent writes may touch: what
/// scores a change (evaluation sets, comparisons, the judge, this module),
/// what guards the agent (its policy, permissions, sandbox, egress filter,
/// quarantine, verification and journal), what holds money and keys, and
/// the build's configuration, whatever the rules a caller gives allow.
pub const PROTECTED: &[&str] = &[
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain",
    "rust-toolchain.toml",
    ".cargo/",
    ".github/",
    "build.rs",
    "crates/sion-agent/src/evals.rs",
    "crates/sion-agent/src/compare.rs",
    "crates/sion-agent/src/judge.rs",
    "crates/sion-agent/src/guard.rs",
    "crates/sion-agent/src/archive.rs",
    "crates/sion-agent/src/remote.rs",
    "crates/sion-agent/src/policy.rs",
    "crates/sion-agent/src/permissions.rs",
    "crates/sion-agent/src/sandbox.rs",
    "crates/sion-agent/src/egress.rs",
    "crates/sion-agent/src/quarantine.rs",
    "crates/sion-agent/src/verify.rs",
    "crates/sion-agent/src/journal.rs",
    "crates/sion-agent/src/oauth.rs",
    "crates/sion-cli/",
    "crates/sion-core/",
    "crates/sion-crypto/",
    "crates/sion-exec/",
    "crates/sion-consensus/",
    "crates/sion-enclave/",
];

impl Default for PatchRules {
    /// The agent's own runtime, apart from what [`PROTECTED`] names: at
    /// most 8 files and 400 lines.
    fn default() -> PatchRules {
        PatchRules {
            allowed: vec!["crates/sion-agent/src/".into()],
            protected: Vec::new(),
            max_files: 8,
            max_lines: 400,
        }
    }
}

/// A file a patch changes, as `git diff --numstat` reports it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Changed {
    /// Its path, relative to the source's root.
    pub path: String,
    /// Lines added.
    pub added: usize,
    /// Lines removed.
    pub removed: usize,
    /// Whether it is a binary file, whose lines are not counted.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub binary: bool,
}

/// Reads `git diff --numstat` output: a changed file a line, a binary
/// file marked as one (git counts no lines of it).
pub fn numstat(output: &str) -> Vec<Changed> {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.splitn(3, '\t');
            let added = fields.next()?;
            let removed = fields.next()?;
            let path = fields.next()?.trim();
            if path.is_empty() {
                return None;
            }
            let binary = added.trim() == "-" || removed.trim() == "-";
            let count = |n: &str| n.trim().parse::<usize>().unwrap_or(0);
            Some(Changed {
                path: path.to_string(),
                added: count(added),
                removed: count(removed),
                binary,
            })
        })
        .collect()
}

/// Whether `path` is `prefix`, or inside it when `prefix` ends with `/`.
fn under(path: &str, prefix: &str) -> bool {
    if prefix.ends_with('/') {
        path.starts_with(prefix)
    } else {
        path == prefix || path.ends_with(&format!("/{prefix}"))
    }
}

/// Whether a patch that changes `changed` keeps to `rules`, and to what
/// [`PROTECTED`] names whatever the rules say; if not, why.
pub fn check_patch(changed: &[Changed], rules: &PatchRules) -> Result<(), String> {
    if changed.is_empty() {
        return Err("the patch changes nothing".into());
    }
    if changed.len() > rules.max_files {
        return Err(format!(
            "the patch changes {} files, more than {}",
            changed.len(),
            rules.max_files
        ));
    }
    let lines: usize = changed.iter().map(|c| c.added + c.removed).sum();
    if lines > rules.max_lines {
        return Err(format!(
            "the patch changes {lines} lines, more than {}",
            rules.max_lines
        ));
    }
    for change in changed {
        let path = change.path.as_str();
        if change.binary {
            return Err(format!(
                "{path}: a binary file, whose size no line count bounds"
            ));
        }
        if path.contains("..") || path.starts_with('/') || path.contains('\\') {
            return Err(format!("{path}: not a path inside the source"));
        }
        // Diffs are read without rename detection: a rename is a removal
        // and an addition, each checked. One shown as `old => new` is not
        // taken.
        if path.contains(" => ") {
            return Err(format!("{path}: a rename is not taken; remove and add"));
        }
        if let Some(rule) = PROTECTED
            .iter()
            .copied()
            .chain(rules.protected.iter().map(String::as_str))
            .find(|p| under(path, p))
        {
            return Err(format!(
                "{path}: the patch may not change {rule}, which scores or guards the agent"
            ));
        }
        if !rules.allowed.iter().any(|a| under(path, a)) {
            return Err(format!(
                "{path}: outside what the patch may change ({})",
                rules.allowed.join(", ")
            ));
        }
    }
    Ok(())
}

/// Whether added lines of a unified diff bring in what the rules of this
/// repository forbid (`unsafe`), or reach for the process's environment or
/// the network in a way a runtime change has no need of; if so, the line.
pub fn forbidden_line(diff: &str) -> Option<String> {
    const FORBIDDEN: &[&str] = &[
        "unsafe {",
        "unsafe{",
        "unsafe fn",
        "unsafe impl",
        "unsafe extern",
        "allow(unsafe",
        "std::env::set_var",
        "std::net::",
        "TcpStream",
        "UdpSocket",
        "std::process::Command",
        "Command::new(",
        "include_bytes!(\"/",
        "include_str!(\"/",
    ];
    diff.lines()
        .filter(|l| l.starts_with('+') && !l.starts_with("+++"))
        .find(|l| FORBIDDEN.iter().any(|f| l.contains(f)))
        .map(|l| l.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evals::{Answer, Fixture};
    use crate::tools::Effect;

    fn sealed() -> Vec<Case> {
        vec![
            Case {
                id: "refund".into(),
                task: "A customer asks for a refund of order 1182 because the parcel arrived crushed and wet".into(),
                tools: vec![Fixture {
                    name: "orders".into(),
                    description: String::new(),
                    effect: Effect::Read,
                    answers: vec![Answer {
                        when: Default::default(),
                        says: "order 1182 was shipped by the slow courier on the ninth of march".into(),
                        error: false,
                        outside: false,
                        confidential: false,
                    }],
                    otherwise: None,
                    command_field: None,
                }],
                checks: vec![Check::Done],
                person: None,
            },
            canary(0, "f3a9c2d17b5e4410"),
        ]
    }

    #[test]
    fn a_proposal_that_repeats_a_sealed_case_is_a_leak() {
        let sealed = sealed();
        let copied = "When a customer asks for a refund of order 1182 because the parcel arrived crushed, refund it.";
        let (case, run) = leak(copied, &sealed).unwrap();
        assert_eq!(case, "refund");
        assert_eq!(run.split(' ').count(), LEAK_WORDS);
        // What a stand-in answers counts too.
        assert!(leak(
            "note: shipped by the slow courier on the ninth of march",
            &sealed
        )
        .is_some());
        // Words in common, but no run of eight: not a leak.
        assert_eq!(
            leak("Check the order before you refund a customer.", &sealed),
            None
        );
        // A canary's code word, anywhere, is.
        let (case, word) = leak("Always answer F3A9C2D17B5E4410.", &sealed).unwrap();
        assert_eq!(
            (case.as_str(), word.as_str()),
            ("canary-0", "f3a9c2d17b5e4410")
        );
    }

    #[test]
    fn a_canary_is_known_by_its_name_and_holds_its_word() {
        let case = canary(3, "abc123");
        assert!(is_canary(&case));
        assert_eq!(canary_word(&case).as_deref(), Some("abc123"));
        assert!(!case.task.contains("abc123"));
        assert!(!is_canary(&sealed()[0]));
    }

    #[test]
    fn a_patch_keeps_to_its_paths_and_its_size() {
        let rules = PatchRules::default();
        let fine = numstat("12\t3\tcrates/sion-agent/src/session.rs\n");
        assert_eq!(check_patch(&fine, &rules), Ok(()));
        let scorer = numstat("1\t1\tcrates/sion-agent/src/evals.rs\n");
        assert!(check_patch(&scorer, &rules)
            .unwrap_err()
            .contains("scores or guards"));
        let build = numstat("1\t0\tCargo.toml\n");
        assert!(check_patch(&build, &rules).is_err());
        let outside = numstat("1\t0\tREADME.md\n");
        assert!(check_patch(&outside, &rules)
            .unwrap_err()
            .contains("outside what"));
        let renamed = numstat("0\t0\tcrates/sion-agent/src/{session.rs => policy.rs}\n");
        assert!(check_patch(&renamed, &rules).is_err());
        let big = numstat("500\t0\tcrates/sion-agent/src/session.rs\n");
        assert!(check_patch(&big, &rules).unwrap_err().contains("lines"));
        assert!(check_patch(&[], &rules).is_err());
        let escape = numstat("1\t0\tcrates/sion-agent/src/../../../etc/passwd\n");
        assert!(check_patch(&escape, &rules).is_err());
        let binary = numstat("-\t-\tcrates/sion-agent/src/blob.bin\n");
        assert!(binary[0].binary);
        assert!(check_patch(&binary, &rules).unwrap_err().contains("binary"));
        // A caller's own protected path holds too.
        let mut own = rules.clone();
        own.protected
            .push("crates/sion-agent/src/session.rs".into());
        assert!(check_patch(&fine, &own).is_err());
    }

    #[test]
    fn added_lines_may_not_bring_in_unsafe_code_or_the_network() {
        let diff = "--- a/x.rs\n+++ b/x.rs\n@@\n-let a = 1;\n+let a = unsafe { f() };\n";
        assert!(forbidden_line(diff).unwrap().contains("unsafe"));
        let removed =
            "--- a/x.rs\n+++ b/x.rs\n@@\n-let s = std::net::TcpStream::connect(a);\n+let s = 1;\n";
        assert_eq!(forbidden_line(removed), None);
    }
}
