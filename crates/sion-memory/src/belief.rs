//! Belief as a fold over revisions that only accumulate.
//!
//! Nothing is ever mutated. What a consumer believes about an atom at Pulse
//! `p` is computed from the revision and conflict atoms that target it in
//! the committed log prefixes, as of `p`, of the revisers the consumer's
//! [`BeliefPolicy`] names (the writer itself, curators, adjudicators). Two
//! consumers with different policies can hold different beliefs over the
//! same evidence.
//!
//! # The fold
//!
//! 1. The atom starts in its class's initial state: Simulation atoms
//!    Simulated, Hypothesis atoms Candidate, everything else Accepted.
//! 2. The events are the revision atoms listing the atom among their
//!    targets and the conflict atoms listing it among their members,
//!    written by a reviser in the policy, within that reviser's committed
//!    prefix. Revisions by anyone else are ignored.
//! 3. Events are applied in the policy's order: by default by
//!    `(known_at, writer, seq)`, so the latest wins; with
//!    [`Prefer::Ranked`], lower-ranked revisers' events first, so the
//!    highest-ranked reviser that spoke has the last word.
//! 4. A revision sets the state (and `end_valid`) to its own. A conflict
//!    makes a live atom (Accepted, Candidate or Disputed) Disputed until a
//!    later revision decides; it leaves retired states alone.
//! 5. A Simulation atom can only be retracted or refuted: no revision makes
//!    a simulation fact.
//!
//! To undo a revision, write a newer one; revisions of revisions do not
//! cascade.
//!
//! # The proof
//!
//! [`prove_belief`] bundles, for each reviser with a committed head, the
//! link-map proof of the complete list of atoms that name the target as of
//! that head, every such atom in full with its inclusion proof. A client
//! holding the revisers' heads and commit histories from the ledger checks
//! the bundle with [`verify_belief`] and folds it without the logs. A proof
//! that no authorized retraction exists is the same bundle with no
//! retraction in it.

use std::collections::{BTreeMap, BTreeSet};

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sion_core::hash::Hash32;
use sion_core::ids::{AccountId, Pulse};
use sion_core::merkle::verify_inclusion;

use crate::atom::{AtomClass, MemoryAtom, MemoryAtomId, Statement};
use crate::error::{MemoryError, Result};
use crate::index::keyed::{prove_links, verify_links, LinkProof};
use crate::log::{CommitHistory, Head, WriterLog};

/// What a consumer believes about an atom.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
    Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum BeliefState {
    /// Believed.
    Accepted,
    /// Contested by a conflict that no later revision decided.
    Disputed,
    /// Replaced by newer memory.
    Superseded,
    /// Withdrawn by a reviser.
    Retracted,
    /// Shown to be false.
    Refuted,
    /// A simulation's output; never fact.
    Simulated,
    /// Awaiting review.
    Candidate,
}

impl BeliefState {
    /// The state an atom of this class starts in.
    pub fn initial(class: AtomClass) -> BeliefState {
        match class {
            AtomClass::Simulation => BeliefState::Simulated,
            AtomClass::Hypothesis => BeliefState::Candidate,
            _ => BeliefState::Accepted,
        }
    }

    /// Whether a conflict can still dispute it.
    pub fn is_live(self) -> bool {
        matches!(
            self,
            BeliefState::Accepted | BeliefState::Candidate | BeliefState::Disputed
        )
    }
}

/// How to order authorized revisers' events.
#[derive(
    Clone, Debug, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Prefer {
    /// By `(known_at, writer, seq)`: the latest event wins.
    #[default]
    Latest,
    /// By rank, then `(known_at, writer, seq)`: the first reviser listed
    /// outranks the second, and so on.
    Ranked(Vec<AccountId>),
}

/// Whose revisions count, and how to order them.
#[derive(
    Clone, Debug, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub struct BeliefPolicy {
    /// The authorized revisers.
    pub revisers: BTreeSet<AccountId>,
    /// The order of their events.
    #[serde(default)]
    pub prefer: Prefer,
}

impl BeliefPolicy {
    /// Latest-wins among `revisers`.
    pub fn new(revisers: impl IntoIterator<Item = AccountId>) -> Self {
        BeliefPolicy {
            revisers: revisers.into_iter().collect(),
            prefer: Prefer::Latest,
        }
    }

    /// Only the writer revises its own memory.
    pub fn only(writer: AccountId) -> Self {
        BeliefPolicy::new([writer])
    }

    /// Revisers ranked from highest to lowest.
    pub fn ranked(order: Vec<AccountId>) -> Self {
        BeliefPolicy {
            revisers: order.iter().copied().collect(),
            prefer: Prefer::Ranked(order),
        }
    }

    fn check(&self) -> Result<()> {
        if let Prefer::Ranked(order) = &self.prefer {
            let unique: BTreeSet<&AccountId> = order.iter().collect();
            if unique.len() != order.len() || order.iter().any(|r| !self.revisers.contains(r)) {
                return Err(MemoryError::InvalidBelief(
                    "a ranking must list distinct authorized revisers",
                ));
            }
        }
        Ok(())
    }

    /// Larger for higher rank; equal for every reviser under `Latest`.
    fn rank(&self, writer: &AccountId) -> usize {
        match &self.prefer {
            Prefer::Latest => 0,
            Prefer::Ranked(order) => order
                .iter()
                .position(|r| r == writer)
                .map_or(0, |i| order.len() - i),
        }
    }
}

/// The outcome of the fold.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
    Deserialize,
)]
pub struct Belief {
    /// The state.
    pub state: BeliefState,
    /// The revision or conflict that decided it (`None`: the initial state).
    pub decided_by: Option<MemoryAtomId>,
    /// When the target stopped holding, if the deciding revision says so.
    pub end_valid: Option<u64>,
}

/// A reviser's log with the ledger's commits of it.
#[derive(Clone, Copy, Debug)]
pub struct ReviserLog<'a> {
    /// The log.
    pub log: &'a WriterLog,
    /// Its commits; the fold reads the committed prefix only.
    pub commits: &'a CommitHistory,
}

impl ReviserLog<'_> {
    fn committed_size(&self) -> Result<u64> {
        let size = self.commits.committed_size();
        if size > self.log.len() {
            return Err(MemoryError::InvalidBelief(
                "commits cover atoms the log lacks",
            ));
        }
        Ok(size)
    }
}

/// A reviser's head and commit history, as the ledger records them.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct ReviserHead {
    /// The reviser.
    pub writer: AccountId,
    /// Its last committed head.
    pub head: Head,
    /// Its commits (the last one's size is the head's).
    pub commits: CommitHistory,
}

struct Event<'a> {
    rank: usize,
    known_at: Pulse,
    writer: AccountId,
    seq: u64,
    id: MemoryAtomId,
    statement: &'a Statement,
}

fn names(statement: &Statement, target: &MemoryAtomId) -> bool {
    match statement {
        Statement::Revision { targets, .. } => targets.binary_search(target).is_ok(),
        Statement::Conflict { members, .. } => members.binary_search(target).is_ok(),
        _ => false,
    }
}

fn fold(target: &MemoryAtom, mut events: Vec<Event<'_>>) -> Belief {
    events.sort_by_key(|e| (e.rank, e.known_at, e.writer, e.seq));
    let simulation = target.class == AtomClass::Simulation;
    let mut belief = Belief {
        state: target.initial_belief(),
        decided_by: None,
        end_valid: None,
    };
    for event in events {
        match event.statement {
            Statement::Revision {
                state, end_valid, ..
            } => {
                if simulation && !matches!(state, BeliefState::Retracted | BeliefState::Refuted) {
                    continue;
                }
                belief = Belief {
                    state: *state,
                    decided_by: Some(event.id),
                    end_valid: *end_valid,
                };
            }
            Statement::Conflict { .. } if belief.state.is_live() => {
                belief = Belief {
                    state: BeliefState::Disputed,
                    decided_by: Some(event.id),
                    end_valid: None,
                };
            }
            _ => {}
        }
    }
    belief
}

fn unique_revisers(writers: impl IntoIterator<Item = AccountId>) -> Result<()> {
    let mut seen = BTreeSet::new();
    for writer in writers {
        if !seen.insert(writer) {
            return Err(MemoryError::InvalidBelief("a reviser appears twice"));
        }
    }
    Ok(())
}

/// What `policy` believes about `target`, folding the revision and conflict
/// atoms in the committed prefixes of the authorized revisers' logs. Logs of
/// writers outside the policy are ignored.
pub fn belief_at(
    target: &MemoryAtom,
    policy: &BeliefPolicy,
    logs: &[ReviserLog<'_>],
) -> Result<Belief> {
    policy.check()?;
    unique_revisers(logs.iter().map(|l| l.log.writer()))?;
    let target_id = target.id();
    let mut events = Vec::new();
    for reviser in logs {
        let writer = reviser.log.writer();
        if !policy.revisers.contains(&writer) {
            continue;
        }
        let size = reviser.committed_size()?;
        for (atom, id) in reviser.log.atoms()[..size as usize]
            .iter()
            .zip(reviser.log.ids())
        {
            if names(&atom.statement, &target_id) {
                events.push(Event {
                    rank: policy.rank(&writer),
                    known_at: reviser.commits.known_at(atom.seq).expect("committed"),
                    writer,
                    seq: atom.seq,
                    id: *id,
                    statement: &atom.statement,
                });
            }
        }
    }
    Ok(fold(target, events))
}

/// An atom that names the target, with its inclusion in the reviser's log.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct LinkedAtom {
    /// The atom.
    pub atom: MemoryAtom,
    /// Its inclusion proof against the head's root.
    pub inclusion: Vec<Hash32>,
}

/// One reviser's evidence about the target.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct ReviserEvidence {
    /// The reviser.
    pub writer: AccountId,
    /// The committed head the evidence is against.
    pub head: Head,
    /// The complete list of atoms naming the target, as of the head.
    pub links: LinkProof,
    /// Those atoms, in the list's order.
    pub linked: Vec<LinkedAtom>,
}

/// Everything needed to fold a belief without the logs.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct BeliefProof {
    /// The target's id.
    pub target: MemoryAtomId,
    /// One entry per authorized reviser with a committed head, by account id.
    pub revisers: Vec<ReviserEvidence>,
}

/// Builds the bundle for `target` from the authorized revisers' logs (each
/// as of its last commit).
pub fn prove_belief(
    target: &MemoryAtom,
    policy: &BeliefPolicy,
    logs: &[ReviserLog<'_>],
) -> Result<BeliefProof> {
    policy.check()?;
    unique_revisers(logs.iter().map(|l| l.log.writer()))?;
    let target_id = target.id();
    let mut by_writer: BTreeMap<AccountId, ReviserEvidence> = BTreeMap::new();
    for reviser in logs {
        let writer = reviser.log.writer();
        if !policy.revisers.contains(&writer) || reviser.commits.commits().is_empty() {
            continue;
        }
        let size = reviser.committed_size()?;
        let index = reviser.log.index(size)?;
        let head = Head {
            size,
            root: reviser.log.root_at(size)?,
            index_root: index.root(),
        };
        let (ids, links) = prove_links(&index, &target_id);
        let mut linked = Vec::with_capacity(ids.len());
        for id in ids {
            let seq = reviser
                .log
                .seq_of(&id)
                .ok_or(MemoryError::UnknownAtom(id))?;
            linked.push(LinkedAtom {
                atom: reviser
                    .log
                    .atom(seq)
                    .expect("indexed atoms are in the log")
                    .clone(),
                inclusion: reviser.log.inclusion_proof_at(seq, size)?,
            });
        }
        by_writer.insert(
            writer,
            ReviserEvidence {
                writer,
                head,
                links,
                linked,
            },
        );
    }
    Ok(BeliefProof {
        target: target_id,
        revisers: by_writer.into_values().collect(),
    })
}

/// Checks a belief bundle against the revisers' heads from the ledger and
/// folds it. Every authorized reviser in `heads` with at least one commit
/// must have evidence against exactly its head; revisers without a commit
/// have none.
pub fn verify_belief(
    target: &MemoryAtom,
    policy: &BeliefPolicy,
    heads: &[ReviserHead],
    proof: &BeliefProof,
) -> Result<Belief> {
    policy.check()?;
    unique_revisers(heads.iter().map(|h| h.writer))?;
    let target_id = target.id();
    if proof.target != target_id {
        return Err(MemoryError::InvalidProof("bundle is for another atom"));
    }
    let expected: Vec<&ReviserHead> = {
        let mut authorized: Vec<&ReviserHead> = heads
            .iter()
            .filter(|h| policy.revisers.contains(&h.writer) && !h.commits.commits().is_empty())
            .collect();
        authorized.sort_by_key(|h| h.writer);
        authorized
    };
    if expected.len() != proof.revisers.len() {
        return Err(MemoryError::InvalidProof(
            "evidence does not cover the authorized revisers",
        ));
    }
    let mut events = Vec::new();
    for (trusted, evidence) in expected.iter().zip(&proof.revisers) {
        if trusted.writer != evidence.writer || trusted.head != evidence.head {
            return Err(MemoryError::InvalidProof(
                "evidence is not against the ledger's head",
            ));
        }
        if trusted.head.size != trusted.commits.committed_size() {
            return Err(MemoryError::InvalidBelief("head and commits disagree"));
        }
        let head = &trusted.head;
        let ids: Vec<MemoryAtomId> = evidence.linked.iter().map(|l| l.atom.id()).collect();
        verify_links(&head.index_root, &target_id, &ids, &evidence.links)?;
        crate::index::keyed::check_size(&evidence.links, head.size)?;
        for (linked, id) in evidence.linked.iter().zip(&ids) {
            let atom = &linked.atom;
            if atom.writer != trusted.writer || atom.seq >= head.size {
                return Err(MemoryError::InvalidProof(
                    "linked atom outside the reviser's head",
                ));
            }
            atom.validate()?;
            verify_inclusion(
                &head.root,
                head.size,
                atom.seq,
                &id.leaf(),
                &linked.inclusion,
            )?;
            if names(&atom.statement, &target_id) {
                events.push(Event {
                    rank: policy.rank(&atom.writer),
                    known_at: trusted
                        .commits
                        .known_at(atom.seq)
                        .ok_or(MemoryError::InvalidBelief("atom beyond the commits"))?,
                    writer: atom.writer,
                    seq: atom.seq,
                    id: *id,
                    statement: &atom.statement,
                });
            }
        }
    }
    Ok(fold(target, events))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atom::{ConflictKind, MemoryKind, Value};
    use crate::geom::Interval;
    use crate::log::Commit;
    use proptest::prelude::*;
    use rand_core::RngCore;

    fn account(name: &str) -> AccountId {
        AccountId::derive(name.as_bytes())
    }

    struct World {
        target: MemoryAtom,
        logs: Vec<WriterLog>,
        commits: Vec<CommitHistory>,
    }

    impl World {
        fn new(class: AtomClass, names: &[&str]) -> World {
            let writer = account(names[0]);
            let target = MemoryAtom::builder(
                writer,
                0,
                MemoryKind::WorldState,
                class,
                Statement::claim("door-4", "state", Value::Text("open".into())),
            )
            .valid(Interval::starting(100))
            .build()
            .unwrap();
            let mut logs: Vec<WriterLog> =
                names.iter().map(|n| WriterLog::new(account(n))).collect();
            logs[0].append(target.clone()).unwrap();
            let commits = names.iter().map(|_| CommitHistory::default()).collect();
            World {
                target,
                logs,
                commits,
            }
        }

        fn revise(&mut self, who: usize, state: BeliefState) -> MemoryAtomId {
            let log = &mut self.logs[who];
            let atom = MemoryAtom::builder(
                log.writer(),
                log.len(),
                MemoryKind::Correction,
                AtomClass::Revision,
                Statement::revision([self.target.id()], state, "review"),
            )
            .inherit_place(&self.target)
            .build()
            .unwrap();
            let id = atom.id();
            log.append(atom).unwrap();
            id
        }

        fn conflict(&mut self, who: usize) -> MemoryAtomId {
            let other = MemoryAtomId(Hash32([0x42; 32]));
            let log = &mut self.logs[who];
            let atom = MemoryAtom::builder(
                log.writer(),
                log.len(),
                MemoryKind::WorldState,
                AtomClass::Conflict,
                Statement::conflict([self.target.id(), other], ConflictKind::Existence),
            )
            .build()
            .unwrap();
            let id = atom.id();
            log.append(atom).unwrap();
            id
        }

        fn commit(&mut self, who: usize, pulse: u64) {
            let size = self.logs[who].len();
            self.commits[who]
                .push(Commit {
                    pulse: Pulse(pulse),
                    size,
                })
                .unwrap();
        }

        fn revisers(&self) -> Vec<ReviserLog<'_>> {
            self.logs
                .iter()
                .zip(&self.commits)
                .map(|(log, commits)| ReviserLog { log, commits })
                .collect()
        }

        fn heads(&self) -> Vec<ReviserHead> {
            self.logs
                .iter()
                .zip(&self.commits)
                .filter(|(_, c)| !c.commits().is_empty())
                .map(|(log, commits)| ReviserHead {
                    writer: log.writer(),
                    head: log.head(commits.committed_size()).unwrap(),
                    commits: commits.clone(),
                })
                .collect()
        }

        fn belief(&self, policy: &BeliefPolicy) -> Belief {
            let direct = belief_at(&self.target, policy, &self.revisers()).unwrap();
            let proof = prove_belief(&self.target, policy, &self.revisers()).unwrap();
            let verified = verify_belief(&self.target, policy, &self.heads(), &proof).unwrap();
            assert_eq!(direct, verified, "the bundle folds to the same belief");
            direct
        }
    }

    #[test]
    fn latest_authorized_revision_wins() {
        let mut w = World::new(AtomClass::Observation, &["robot", "curator", "stranger"]);
        w.commit(0, 1);
        let policy = BeliefPolicy::new([account("robot"), account("curator")]);
        assert_eq!(w.belief(&policy).state, BeliefState::Accepted);
        let retract = w.revise(1, BeliefState::Retracted);
        w.commit(1, 5);
        let b = w.belief(&policy);
        assert_eq!(
            (b.state, b.decided_by),
            (BeliefState::Retracted, Some(retract))
        );
        let accept = w.revise(0, BeliefState::Accepted);
        w.commit(0, 7);
        assert_eq!(w.belief(&policy).decided_by, Some(accept));
        // A stranger's revision is ignored.
        w.revise(2, BeliefState::Refuted);
        w.commit(2, 9);
        assert_eq!(w.belief(&policy).state, BeliefState::Accepted);
        // Uncommitted revisions do not count yet.
        w.revise(1, BeliefState::Superseded);
        assert_eq!(w.belief(&policy).state, BeliefState::Accepted);
        w.commit(1, 11);
        assert_eq!(w.belief(&policy).state, BeliefState::Superseded);
        // A consumer that trusts only the robot sees its own revision.
        assert_eq!(
            w.belief(&BeliefPolicy::only(account("robot"))).decided_by,
            Some(accept)
        );
    }

    #[test]
    fn conflicts_dispute_until_decided() {
        let mut w = World::new(AtomClass::Observation, &["robot", "curator"]);
        w.commit(0, 1);
        let policy = BeliefPolicy::new([account("robot"), account("curator")]);
        let conflict = w.conflict(1);
        w.commit(1, 2);
        let b = w.belief(&policy);
        assert_eq!(
            (b.state, b.decided_by),
            (BeliefState::Disputed, Some(conflict))
        );
        w.revise(1, BeliefState::Accepted);
        w.commit(1, 3);
        assert_eq!(w.belief(&policy).state, BeliefState::Accepted);
        // A conflict does not revive a retracted atom.
        w.revise(0, BeliefState::Retracted);
        w.commit(0, 4);
        w.conflict(1);
        w.commit(1, 5);
        assert_eq!(w.belief(&policy).state, BeliefState::Retracted);
    }

    #[test]
    fn simulations_never_become_fact_and_hypotheses_start_candidate() {
        let mut sim = World::new(AtomClass::Simulation, &["planner"]);
        sim.commit(0, 1);
        let policy = BeliefPolicy::only(account("planner"));
        assert_eq!(sim.belief(&policy).state, BeliefState::Simulated);
        sim.revise(0, BeliefState::Accepted);
        sim.conflict(0);
        sim.commit(0, 2);
        assert_eq!(sim.belief(&policy).state, BeliefState::Simulated);
        sim.revise(0, BeliefState::Retracted);
        sim.commit(0, 3);
        assert_eq!(sim.belief(&policy).state, BeliefState::Retracted);
        let mut hypothesis = World::new(AtomClass::Hypothesis, &["agent"]);
        hypothesis.commit(0, 1);
        let policy = BeliefPolicy::only(account("agent"));
        assert_eq!(hypothesis.belief(&policy).state, BeliefState::Candidate);
        hypothesis.revise(0, BeliefState::Accepted);
        hypothesis.commit(0, 2);
        assert_eq!(hypothesis.belief(&policy).state, BeliefState::Accepted);
    }

    #[test]
    fn ranked_revisers_outrank_later_ones() {
        let mut w = World::new(AtomClass::Observation, &["robot", "curator", "judge"]);
        w.commit(0, 1);
        let judged = w.revise(2, BeliefState::Refuted);
        w.commit(2, 2);
        w.revise(1, BeliefState::Accepted);
        w.commit(1, 3);
        let latest = BeliefPolicy::new([account("curator"), account("judge")]);
        assert_eq!(w.belief(&latest).state, BeliefState::Accepted);
        let ranked = BeliefPolicy::ranked(vec![account("judge"), account("curator")]);
        assert_eq!(w.belief(&ranked).decided_by, Some(judged));
        let mut bad = ranked.clone();
        bad.revisers.remove(&account("judge"));
        assert!(belief_at(&w.target, &bad, &w.revisers()).is_err());
    }

    #[test]
    fn bundles_reject_tampering() {
        let mut w = World::new(AtomClass::Observation, &["robot", "curator"]);
        w.commit(0, 1);
        w.revise(1, BeliefState::Retracted);
        w.commit(1, 2);
        let policy = BeliefPolicy::new([account("robot"), account("curator")]);
        let proof = prove_belief(&w.target, &policy, &w.revisers()).unwrap();
        let heads = w.heads();
        assert_eq!(
            verify_belief(&w.target, &policy, &heads, &proof)
                .unwrap()
                .state,
            BeliefState::Retracted
        );
        // Hiding the retraction fails: the link list is complete.
        let mut hidden = proof.clone();
        let curator = hidden
            .revisers
            .iter_mut()
            .find(|e| e.writer == account("curator"))
            .unwrap();
        curator.linked.clear();
        assert!(verify_belief(&w.target, &policy, &heads, &hidden).is_err());
        // Dropping a reviser's evidence fails.
        let mut dropped = proof.clone();
        dropped.revisers.pop();
        assert!(verify_belief(&w.target, &policy, &heads, &dropped).is_err());
        // Evidence against a stale head fails.
        let mut stale = heads.clone();
        stale[0].head.size = 0;
        assert!(verify_belief(&w.target, &policy, &stale, &proof).is_err());
        // A bundle for another atom fails.
        let mut other = w.target.clone();
        other.confidence = 1;
        assert!(verify_belief(&other, &policy, &heads, &proof).is_err());
        // Duplicate revisers are refused.
        let mut twice = heads.clone();
        twice.push(heads[0].clone());
        assert!(verify_belief(&w.target, &policy, &twice, &proof).is_err());
    }

    proptest! {
        #![proptest_config(crate::testing::config(96, 0x5eed_b311))]

        #[test]
        fn fold_matches_a_reference_and_its_bundle(seed in any::<u64>(), steps in 0usize..24) {
            let names = ["robot", "curator", "judge", "stranger"];
            let mut w = World::new(AtomClass::Observation, &names);
            w.commit(0, 0);
            let policy = BeliefPolicy::new([account("robot"), account("curator"), account("judge")]);
            let mut rng = sion_core::rng::seeded(seed);
            // Reference: (pulse, writer, seq, event) of authorized, committed events.
            let mut pulse = 1u64;
            let mut pending: Vec<Vec<(u64, Option<BeliefState>, MemoryAtomId)>> = vec![Vec::new(); 4];
            let mut reference: Vec<(Pulse, AccountId, u64, Option<BeliefState>, MemoryAtomId)> = Vec::new();
            let states = [BeliefState::Accepted, BeliefState::Superseded, BeliefState::Retracted, BeliefState::Refuted, BeliefState::Candidate];
            for _ in 0..steps {
                let who = (rng.next_u32() % 4) as usize;
                match rng.next_u32() % 3 {
                    0 => {
                        let state = states[(rng.next_u32() % 5) as usize];
                        let seq = w.logs[who].len();
                        let id = w.revise(who, state);
                        pending[who].push((seq, Some(state), id));
                    }
                    1 => {
                        let seq = w.logs[who].len();
                        let id = w.conflict(who);
                        pending[who].push((seq, None, id));
                    }
                    _ => {
                        pulse += rng.next_u64() % 3;
                        w.commit(who, pulse);
                        if who != 3 {
                            for (seq, event, id) in pending[who].drain(..) {
                                reference.push((Pulse(pulse), account(names[who]), seq, event, id));
                            }
                        } else {
                            pending[who].clear();
                        }
                    }
                }
            }
            reference.sort_by_key(|(p, w, s, _, _)| (*p, *w, *s));
            let mut expected = (BeliefState::Accepted, None);
            for (_, _, _, event, id) in reference {
                match event {
                    Some(state) => expected = (state, Some(id)),
                    None => {
                        if matches!(expected.0, BeliefState::Accepted | BeliefState::Candidate | BeliefState::Disputed) {
                            expected = (BeliefState::Disputed, Some(id));
                        }
                    }
                }
            }
            let belief = w.belief(&policy);
            prop_assert_eq!((belief.state, belief.decided_by), expected);
        }
    }
}
