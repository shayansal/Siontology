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
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            autonomy: Autonomy::L3,
            grants: BTreeSet::new(),
            approval_ttl: 15 * 60,
            taint_ttl: 24 * 3600,
        }
    }
}

/// A session's standing: the approvals it was given and its taint.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Standing {
    /// Approvals: a call's key and when its approval expires (seconds).
    pub approvals: BTreeMap<String, u64>,
    /// Until when the session is tainted (seconds), if it is.
    pub tainted_until: Option<u64>,
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
}

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
        if decision == Decision::Run && effect >= Effect::External && standing.tainted(now) {
            decision = Decision::Ask(
                "the session has read content from outside; external calls and spending need \
                 approval until the taint expires"
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
