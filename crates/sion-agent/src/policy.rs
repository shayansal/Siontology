//! Whether a call runs: autonomy levels, grants, approvals and taint.
//!
//! Spending limits are not here. The ledger enforces the agent's charter
//! (payees, per-payment caps, its token bucket, budget, expiry) whatever the
//! runtime decides, so a runtime that is misconfigured or compromised still
//! cannot spend beyond it.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::tools::Effect;

/// How much an agent may do without asking, from L0 (nothing) to L6
/// (everything its charter allows).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Autonomy {
    /// Nothing runs.
    L0,
    /// Reads run; nothing else.
    L1,
    /// Reads run; writes need approval.
    L2,
    /// Reads and writes run; nothing reaches outside.
    L3,
    /// Reads and writes run; external calls need approval; no spending.
    L4,
    /// Reads, writes and external calls run; spending needs approval.
    L5,
    /// Everything runs; spending stays within the charter.
    L6,
}

impl std::str::FromStr for Autonomy {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Ok(match text.to_ascii_uppercase().as_str() {
            "L0" | "0" => Autonomy::L0,
            "L1" | "1" => Autonomy::L1,
            "L2" | "2" => Autonomy::L2,
            "L3" | "3" => Autonomy::L3,
            "L4" | "4" => Autonomy::L4,
            "L5" | "5" => Autonomy::L5,
            "L6" | "6" => Autonomy::L6,
            _ => return Err(format!("autonomy is L0 to L6, not {text}")),
        })
    }
}

/// The decision about one call.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Decision {
    /// Run it.
    Run,
    /// Ask a person first, for this reason.
    Ask(String),
    /// Do not run it, for this reason.
    Deny(String),
}

impl Autonomy {
    /// What this level decides for an effect, before grants, taint and
    /// approvals.
    pub fn decide(self, effect: Effect) -> Decision {
        use Autonomy::*;
        use Effect::*;
        let deny = |what: &str| Decision::Deny(format!("autonomy {self:?} allows no {what}"));
        let ask = |what: &str| Decision::Ask(format!("{what} at autonomy {self:?}"));
        match (self, effect) {
            (L0, _) => deny("calls"),
            (_, Read) => Decision::Run,
            (L1, Write) => deny("writes"),
            (L1 | L2 | L3, External) => deny("external calls"),
            (L1 | L2 | L3 | L4, Spend) => deny("spending"),
            (L2, Write) => ask("a write"),
            (_, Write) => Decision::Run,
            (L4, External) => ask("an external call"),
            (L5, Spend) => ask("spending"),
            (L5 | L6, External) | (L6, Spend) => Decision::Run,
        }
    }
}

/// The rules a session runs under.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Policy {
    /// The autonomy level.
    pub autonomy: Autonomy,
    /// The tools granted, by name; empty means every tool of the session.
    pub grants: BTreeSet<String>,
    /// How long an approval lasts, in seconds.
    pub approval_ttl: u64,
    /// How long content from outside taints a session, in seconds.
    pub taint_ttl: u64,
    /// Whether the skills a session read may pre-approve calls with their
    /// `allowed-tools` (see [`crate::permissions`]).
    #[serde(default = "yes")]
    pub skill_approvals: bool,
    /// Tools whose external calls reach only where the operator named (a
    /// browser held to its hosts): content from outside does not hold
    /// them for approval. Spending is held whatever the tool.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub contained: BTreeSet<String>,
    /// Whether content from outside is held out of the conversation and
    /// read through a reader without tools ([`crate::quarantine`]).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub quarantine: bool,
    /// Calls that always wait for a person, whatever the autonomy level, a
    /// skill or an allowance says (`ledger_pay`, `shell(git push:*)`), as
    /// calls that cannot be undone do ([`crate::tools::Tool::irreversible`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub always_ask: Vec<crate::permissions::ToolRule>,
}

fn yes() -> bool {
    true
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            autonomy: Autonomy::L3,
            grants: BTreeSet::new(),
            approval_ttl: 15 * 60,
            taint_ttl: 24 * 3600,
            skill_approvals: true,
            contained: BTreeSet::new(),
            quarantine: false,
            always_ask: Vec::new(),
        }
    }
}

/// Why a shell command cannot be undone, if it plainly cannot: what it
/// destroys, or what it sends that cannot be called back. The list is of
/// commands, not of everything a program could do: a script that deletes
/// files is not seen here.
pub fn irreversible_command(command: &str) -> Option<String> {
    // The command's words as the shell splits them: what is quoted is one
    // word, so that a command quoted as text is not taken for one run.
    let mut words: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut quote: Option<char> = None;
    for c in command.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => word.push(c),
            (None, '\'' | '"') => quote = Some(c),
            (None, c) if c.is_whitespace() || matches!(c, ';' | '&' | '|' | '(' | ')') => {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            }
            (None, c) => word.push(c),
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    let lower = command.to_ascii_lowercase();
    let has = |w: &str| words.iter().any(|x| x == w);
    let flag = |short: char, long: &str| {
        words.iter().any(|x| {
            x == long || (x.starts_with('-') && !x.starts_with("--") && x[1..].contains(short))
        })
    };
    // Each command of a line: the words after a separator.
    let starts = |cmd: &str, sub: &str| {
        words
            .windows(2)
            .any(|pair| pair[0] == cmd && (sub.is_empty() || pair[1] == sub))
            || (sub.is_empty() && has(cmd))
    };
    if starts("git", "push")
        && (flag('f', "--force")
            || words
                .iter()
                .any(|w| w.starts_with("--force") || w.starts_with("+"))
            || has("--delete")
            || has("--mirror")
            || words.iter().any(|w| w.starts_with(':') && w.len() > 1))
    {
        return Some(
            "it rewrites or deletes what a remote holds (git push --force or --delete)".into(),
        );
    }
    if starts("git", "reset") && has("--hard") {
        return Some("it throws away changes not committed (git reset --hard)".into());
    }
    if starts("git", "clean") && flag('f', "--force") {
        return Some("it deletes files git does not track (git clean -f)".into());
    }
    if starts("git", "branch") && (has("-D") || (flag('d', "--delete") && flag('f', "--force"))) {
        return Some("it deletes a branch whatever it holds (git branch -D)".into());
    }
    if starts("rm", "") && flag('r', "--recursive") && flag('f', "--force") {
        let wide = words.iter().any(|w| {
            matches!(
                w.as_str(),
                "/" | "/*" | "~" | "~/" | "~/*" | "." | "./" | "./*" | "*" | ".*" | ".." | "../*"
            ) || w.starts_with("$HOME")
                || w.starts_with("/workspace")
        });
        if wide {
            return Some("it deletes a whole directory tree for good (rm -rf)".into());
        }
    }
    for (program, why) in [
        ("mkfs", "it formats a file system"),
        ("shred", "it destroys files beyond recovery"),
        ("wipefs", "it erases a disk's signatures"),
    ] {
        if words
            .iter()
            .any(|w| w == program || w.starts_with(&format!("{program}.")))
        {
            return Some(why.into());
        }
    }
    if starts("dd", "") && words.iter().any(|w| w.starts_with("of=/dev/")) {
        return Some("it writes over a device (dd of=/dev/…)".into());
    }
    for (phrase, why) in [
        ("drop database", "it drops a database"),
        ("drop table", "it drops a table"),
        ("drop schema", "it drops a schema"),
        ("truncate table", "it empties a table"),
        (
            "terraform destroy",
            "it destroys the infrastructure terraform manages",
        ),
        ("kubectl delete", "it deletes what a cluster runs"),
        ("gh repo delete", "it deletes a repository"),
        ("aws s3 rb", "it removes a bucket"),
        ("aws s3 rm", "it deletes objects from a bucket"),
        (
            "docker system prune",
            "it deletes images, containers and volumes",
        ),
        ("docker volume rm", "it deletes volumes"),
    ] {
        let spaced = lower.split_whitespace().collect::<Vec<_>>().join(" ");
        if spaced.contains(phrase) {
            return Some(why.into());
        }
    }
    if lower.contains("delete from") && !lower.contains(" where ") {
        return Some("it deletes every row of a table (DELETE without WHERE)".into());
    }
    None
}

/// A session's standing: the approvals it was given and its taint.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Standing {
    /// Approvals: a call's key and when its approval expires (seconds).
    pub approvals: BTreeMap<String, u64>,
    /// Until when the session is tainted (seconds), if it is.
    pub tainted_until: Option<u64>,
    /// The calls the skills the session read pre-approve: each skill's
    /// `allowed-tools` rules, by its name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pre_approved: BTreeMap<String, Vec<crate::permissions::ToolRule>>,
    /// The calls people allowed for the rest of the session, when they
    /// approved one ([`crate::session::Scope`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed: Vec<Allowance>,
    /// Whether the session holds confidential content: from then on, for
    /// the rest of it, its external calls and spending wait for a person,
    /// and nothing a skill or a person allowed before runs them unasked.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub confidential: bool,
}

/// Calls a person allowed for the rest of a session when they approved
/// one: the same call again, or those of its tool a rule matches (as a
/// skill's `allowed-tools` writes it). Like a skill's pre-approval, it
/// never covers spending, and never holds while the session is tainted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Allowance {
    /// The tool.
    pub tool: String,
    /// The exact input allowed, if only that.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<serde_json::Value>,
    /// The rule that matches the calls allowed, if a pattern.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<crate::permissions::ToolRule>,
    /// Who allowed it, as the channel names them.
    pub by: String,
    /// When (seconds).
    pub at: u64,
}

impl Allowance {
    /// Whether it allows calling `tool` with `input`.
    pub fn matches(&self, tool: &str, input: &serde_json::Value) -> bool {
        if self.tool != tool {
            return false;
        }
        match (&self.input, &self.rule) {
            (Some(allowed), _) => allowed == input,
            (None, Some(rule)) => rule.matches(tool, input),
            (None, None) => false,
        }
    }

    /// What it allows, in words.
    pub fn describe(&self) -> String {
        match (&self.input, &self.rule) {
            (Some(input), _) => format!("{} {input}", self.tool),
            (None, Some(rule)) => rule.describe(),
            (None, None) => self.tool.clone(),
        }
    }
}

impl Standing {
    /// Approves the call `key` at `now`.
    pub fn approve(&mut self, policy: &Policy, key: &str, now: u64) {
        self.approvals
            .insert(key.to_string(), now.saturating_add(policy.approval_ttl));
    }

    /// Taints the session from `now`.
    pub fn taint(&mut self, policy: &Policy, now: u64) {
        self.tainted_until = Some(now.saturating_add(policy.taint_ttl));
    }

    /// Whether the session is tainted at `now`.
    pub fn tainted(&self, now: u64) -> bool {
        self.tainted_until.is_some_and(|until| now < until)
    }

    /// Marks the session as holding confidential content
    /// ([`Standing::confidential`]): it does for the rest of it.
    pub fn hold_confidential(&mut self) {
        self.confidential = true;
    }

    /// What the session holds at `now` that its calls carry on.
    pub fn held(&self, now: u64) -> crate::tools::Held {
        crate::tools::Held {
            tainted: self.tainted(now),
            confidential: self.confidential,
        }
    }

    /// Whether what a skill or a person allowed runs calls unasked at
    /// `now`: not while the session is tainted, nor once it holds
    /// confidential content.
    pub fn open(&self, now: u64) -> bool {
        !self.tainted(now) && !self.confidential
    }

    /// The allowance a person gave that covers calling `tool` with
    /// `input`, if one does.
    pub fn allowing(&self, tool: &str, input: &serde_json::Value) -> Option<&Allowance> {
        self.allowed.iter().find(|a| a.matches(tool, input))
    }

    /// The skill whose `allowed-tools` pre-approve calling `tool` with
    /// `input`, and the rule, if one does.
    pub fn pre_approving(
        &self,
        tool: &str,
        input: &serde_json::Value,
    ) -> Option<(&str, &crate::permissions::ToolRule)> {
        self.pre_approved.iter().find_map(|(skill, rules)| {
            rules
                .iter()
                .find(|r| r.matches(tool, input))
                .map(|r| (skill.as_str(), r))
        })
    }
}

/// Tools whose writes later sessions read as instructions: in a tainted
/// session, they wait for a person, since what content from outside steers
/// them into would steer every session after.
pub const LASTING: [&str; 1] = ["skill_write"];

impl Policy {
    /// Whether a call of `tool`, with effect `effect` and key `key`, runs at
    /// `now` (seconds).
    pub fn decide(
        &self,
        standing: &Standing,
        tool: &str,
        effect: Effect,
        key: &str,
        now: u64,
    ) -> Decision {
        if !self.grants.is_empty() && !self.grants.contains(tool) {
            return Decision::Deny(format!("{tool} is not granted to this agent"));
        }
        let mut decision = self.autonomy.decide(effect);
        let contained = effect == Effect::External && self.contained.contains(tool);
        if decision == Decision::Run
            && effect >= Effect::External
            && !contained
            && standing.tainted(now)
        {
            decision = Decision::Ask(
                "the session has read content from outside; external calls and spending need \
                 approval until the taint expires"
                    .into(),
            );
        }
        if decision == Decision::Run && LASTING.contains(&tool) && standing.tainted(now) {
            decision = Decision::Ask(
                "the session has read content from outside, and what it writes as a skill \
                 reaches every later session's instructions: it needs approval until the taint \
                 expires"
                    .into(),
            );
        }
        if decision == Decision::Run
            && effect >= Effect::External
            && !contained
            && standing.confidential
        {
            decision = Decision::Ask(
                "the session holds confidential content; its external calls and spending need \
                 approval"
                    .into(),
            );
        }
        if matches!(decision, Decision::Ask(_))
            && standing
                .approvals
                .get(key)
                .is_some_and(|until| now < *until)
        {
            return Decision::Run;
        }
        decision
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_that_cannot_be_undone_are_known() {
        for (command, word) in [
            ("git push --force origin main", "git push --force"),
            ("git push -f", "git push --force"),
            ("git push origin +main", "git push --force"),
            ("git push origin --delete old", "--delete"),
            ("git push origin :old", "--delete"),
            ("cd repo && git reset --hard HEAD~3", "git reset --hard"),
            ("git clean -fdx", "git clean -f"),
            ("git branch -D feature", "git branch -D"),
            ("rm -rf /workspace", "rm -rf"),
            ("rm -fr ./*", "rm -rf"),
            ("sudo rm -r -f ~", "rm -rf"),
            ("dd if=x.img of=/dev/sda", "dd of"),
            ("mkfs.ext4 /dev/sdb1", "file system"),
            ("psql -c 'DROP TABLE orders'", "drops a table"),
            ("sqlite3 db \"delete from users\"", "DELETE without WHERE"),
            ("terraform destroy -auto-approve", "terraform"),
            ("kubectl delete deployment web", "cluster"),
        ] {
            let why = irreversible_command(command).unwrap_or_else(|| panic!("{command}"));
            assert!(why.contains(word), "{command}: {why}");
        }
        for command in [
            "git push origin main",
            "git status",
            "rm -rf build target/debug",
            "rm notes.txt",
            "cargo test",
            "sqlite3 db \"delete from users where id = 3\"",
            "echo 'git push --force is bad'",
        ] {
            assert_eq!(irreversible_command(command), None, "{command}");
        }
    }

    #[test]
    fn a_tainted_session_writes_no_skill_without_a_person() {
        let policy = Policy {
            autonomy: Autonomy::L6,
            ..Policy::default()
        };
        let mut standing = Standing::default();
        assert_eq!(
            policy.decide(&standing, "skill_write", Effect::Write, "k", 10),
            Decision::Run
        );
        standing.taint(&policy, 5);
        assert!(matches!(
            policy.decide(&standing, "skill_write", Effect::Write, "k", 10),
            Decision::Ask(_)
        ));
        // Other writes go on as before.
        assert_eq!(
            policy.decide(&standing, "write_file", Effect::Write, "k", 10),
            Decision::Run
        );
    }

    #[test]
    fn confidential_content_holds_external_calls_and_spending_for_a_person() {
        let policy = Policy {
            autonomy: Autonomy::L6,
            contained: BTreeSet::from(["browser".to_string()]),
            ..Policy::default()
        };
        let mut standing = Standing::default();
        let decide = |standing: &Standing, tool: &str, effect: Effect| {
            policy.decide(standing, tool, effect, "k", 10)
        };
        assert_eq!(
            decide(&standing, "fetch_url", Effect::External),
            Decision::Run
        );
        standing.hold_confidential();
        assert!(standing.held(10).confidential && !standing.open(10));
        assert_eq!(decide(&standing, "read_file", Effect::Read), Decision::Run);
        assert_eq!(
            decide(&standing, "write_file", Effect::Write),
            Decision::Run
        );
        for (tool, effect) in [
            ("fetch_url", Effect::External),
            ("ledger_pay", Effect::Spend),
        ] {
            let Decision::Ask(why) = decide(&standing, tool, effect) else {
                panic!("{tool} ran");
            };
            assert!(why.contains("confidential content"), "{why}");
        }
        // A tool that reaches only where the operator named is not held,
        // and an approval of the call itself still runs it.
        assert_eq!(
            decide(&standing, "browser", Effect::External),
            Decision::Run
        );
        standing.approve(&policy, "k", 10);
        assert_eq!(
            decide(&standing, "fetch_url", Effect::External),
            Decision::Run
        );
    }

    #[test]
    fn the_ladder_widens_one_effect_at_a_time() {
        use Autonomy::*;
        use Effect::*;
        let run = |a: Autonomy, e: Effect| a.decide(e) == Decision::Run;
        let asks = |a: Autonomy, e: Effect| matches!(a.decide(e), Decision::Ask(_));
        assert!(!run(L0, Read));
        assert!(run(L1, Read) && !run(L1, Write));
        assert!(asks(L2, Write) && !run(L2, External));
        assert!(run(L3, Write) && !run(L3, External) && !asks(L3, External));
        assert!(asks(L4, External) && !run(L4, Spend) && !asks(L4, Spend));
        assert!(run(L5, External) && asks(L5, Spend));
        assert!(run(L6, Spend));
        // Each level allows everything the one below it allows.
        let levels = [L0, L1, L2, L3, L4, L5, L6];
        for pair in levels.windows(2) {
            for e in [Read, Write, External, Spend] {
                assert!(!run(pair[0], e) || run(pair[1], e), "{:?} {e:?}", pair[1]);
            }
        }
    }

    #[test]
    fn taint_turns_external_calls_into_questions_until_it_expires() {
        let policy = Policy {
            autonomy: Autonomy::L6,
            ..Policy::default()
        };
        let mut standing = Standing::default();
        assert_eq!(
            policy.decide(&standing, "pay", Effect::Spend, "k", 100),
            Decision::Run
        );
        standing.taint(&policy, 100);
        assert!(matches!(
            policy.decide(&standing, "pay", Effect::Spend, "k", 101),
            Decision::Ask(_)
        ));
        assert_eq!(
            policy.decide(&standing, "read", Effect::Read, "r", 101),
            Decision::Run
        );
        let later = 100 + policy.taint_ttl;
        assert_eq!(
            policy.decide(&standing, "pay", Effect::Spend, "k", later),
            Decision::Run
        );
    }

    #[test]
    fn a_contained_tool_acts_after_a_taint_and_spending_still_asks() {
        let policy = Policy {
            autonomy: Autonomy::L5,
            contained: ["browser_click".to_string()].into(),
            ..Policy::default()
        };
        let mut standing = Standing::default();
        standing.taint(&policy, 100);
        assert_eq!(
            policy.decide(&standing, "browser_click", Effect::External, "c", 101),
            Decision::Run
        );
        assert!(matches!(
            policy.decide(&standing, "fetch_url", Effect::External, "f", 101),
            Decision::Ask(_)
        ));
        assert!(matches!(
            policy.decide(&standing, "browser_click", Effect::Spend, "s", 101),
            Decision::Ask(_)
        ));
        // What the level refuses, it still refuses.
        let lower = Policy {
            autonomy: Autonomy::L3,
            ..policy
        };
        assert!(matches!(
            lower.decide(&standing, "browser_click", Effect::External, "c", 101),
            Decision::Deny(_)
        ));
    }

    #[test]
    fn an_approval_covers_one_call_and_expires() {
        let policy = Policy {
            autonomy: Autonomy::L5,
            ..Policy::default()
        };
        let mut standing = Standing::default();
        standing.approve(&policy, "pay-1", 1_000);
        assert_eq!(
            policy.decide(&standing, "pay", Effect::Spend, "pay-1", 1_001),
            Decision::Run
        );
        assert!(matches!(
            policy.decide(&standing, "pay", Effect::Spend, "pay-2", 1_001),
            Decision::Ask(_)
        ));
        assert!(matches!(
            policy.decide(
                &standing,
                "pay",
                Effect::Spend,
                "pay-1",
                1_000 + policy.approval_ttl
            ),
            Decision::Ask(_)
        ));
    }

    #[test]
    fn a_tool_not_granted_is_refused_whatever_the_level() {
        let policy = Policy {
            autonomy: Autonomy::L6,
            grants: ["read_file".to_string()].into(),
            ..Policy::default()
        };
        let standing = Standing::default();
        assert!(matches!(
            policy.decide(&standing, "shell", Effect::Read, "k", 0),
            Decision::Deny(_)
        ));
        assert_eq!(
            policy.decide(&standing, "read_file", Effect::Read, "k", 0),
            Decision::Run
        );
    }
}
