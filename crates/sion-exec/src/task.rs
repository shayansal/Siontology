//! Task Charters (specification §5.42 to §5.44): escrowed work whose
//! acceptance is fixed when it is posted.
//!
//! A **task** escrows money for a piece of work, such as an evaluation run,
//! a batch of reinforcement-learning rollouts, a labelled dataset or a code
//! change, and fixes, at posting, the rule a result must pass to be paid:
//!
//! - the **policy** the decision rule runs under (whose assumptions count,
//!   and κ);
//! - the **required claims** every result must rest on, typically the
//!   environment vendor's bonded claim that its environment has no known
//!   exploit that passes the grader without solving the task;
//! - the **verifiers** whose attestations count, those who must attest
//!   every result, and how many attestations a result needs (v1.15);
//! - the **adjudicator** results must name, the largest **detection
//!   factor** they may declare, and how long the poster's reliance on a
//!   paid result lasts.
//!
//! A worker settles a task with a result claim (schema [`RESULT_SCHEMA`])
//! whose payload commits to the task and its output ([`TaskResult`]), and
//! whose formula is exactly [`result_formula`]: the worker's task bond or
//! any listed verifier's attestation of the same output, and every
//! required claim. No result is attested by its own author, and a named
//! worker is never one of its task's verifiers (v1.15). Faking an accepted
//! result therefore costs at least what the decision rule demands, and the
//! ledger reserves κ times the payment
//! of the result's backing for the reliance period. If an exploit in the
//! environment is proven later, the environment's claim is falsified, every
//! result resting on it falls with it, and the poster is compensated from
//! the vendor's slashed bond: a bounty for reward hacks, paid by whoever
//! sold the environment.
//!
//! A task is paid exactly once: settling it closes it. A worker named by a
//! task can count on it until its deadline: only the worker can release it
//! before then. An open task (no worker named) is a bounty that anyone may
//! settle, and its poster may withdraw it until it is settled.
//!
//! An agent posts tasks from its allotment, under its caveats, so a whole
//! tree of agents posting work can never spend more than its root was
//! given. This module defines the types, the constants and
//! [`result_formula`]; the rules are in [`crate::execute`].

use std::collections::BTreeSet;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sion_assurance::{AtomClass, Formula, Policy};
use sion_core::encode::encode;
use sion_core::hash::Hash32;
use sion_core::ids::{AccountId, AtomId, ClaimId, ObjectId, Pulse};
use sion_core::Amount;

use crate::claim::{formulas, Payload};
use crate::execute::bond_atom;

/// The schema of a task's result claims.
pub const RESULT_SCHEMA: &str = "task.result/v1";

/// The schema of verifiers' attestations of a result.
pub const ATTESTATION_SCHEMA: &str = "task.attestation/v1";

/// The label of a worker's task bond: the atom `bond:<worker>:task-worker`.
pub const WORKER_BOND_LABEL: &str = "task-worker";

/// The most claims a task may require of its results.
pub const MAX_REQUIRED: usize = 16;

/// The most verifiers a task may name.
pub const MAX_VERIFIERS: usize = 16;

/// What a result and its attestations commit to: the task, and a
/// commitment to the output kept off the ledger.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub struct TaskResult {
    /// The task.
    pub task: ObjectId,
    /// A commitment to the output (a digest of the output, or the root of a
    /// Merkle tree over a batch of outputs).
    pub output: Hash32,
}

impl TaskResult {
    /// The payload a result claim and its attestations carry.
    pub fn payload(&self) -> Payload {
        Payload::Public(encode(self))
    }
}

/// The rule a result must pass, fixed when the task is posted.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Acceptance {
    /// The poster's policy for the decision rule.
    pub policy: Policy,
    /// Claims every result must rest on.
    pub requires: BTreeSet<ClaimId>,
    /// Accounts whose attestations count.
    pub verifiers: BTreeSet<AccountId>,
    /// Verifiers who must each attest every result (specification v1.15):
    /// a subset of `verifiers`.
    #[serde(default)]
    pub must_attest: BTreeSet<AccountId>,
    /// How many attestations a result needs at least (specification
    /// v1.15), at most the number of verifiers.
    #[serde(default)]
    pub min_attestations: u8,
    /// The adjudicator a result must name, if any.
    pub adjudicator: Option<AtomId>,
    /// The largest detection factor a result may declare, in thousandths.
    pub detection_permille: u16,
    /// How many Pulses the poster's reliance on a paid result lasts: the
    /// reservation of its backing, and with it compensation if it is
    /// proven false.
    pub reliance: u64,
}

/// Where a task stands.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub enum TaskState {
    /// Waiting for an acceptable result.
    Open,
    /// Paid, once and for all.
    Settled {
        /// Who was paid.
        worker: AccountId,
        /// The result claim.
        result: ClaimId,
        /// The reservation of the result's backing.
        reservation: Hash32,
        /// When.
        at: Pulse,
    },
    /// Withdrawn, released or expired, and refunded.
    Cancelled {
        /// Who cancelled it.
        by: AccountId,
        /// When.
        at: Pulse,
    },
}

/// An escrowed task (a shared object in its own contention domain).
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Task {
    /// Who posted it.
    pub poster: AccountId,
    /// The agent's charter whose allotment funded it, if any.
    pub funder: Option<ObjectId>,
    /// Who receives the escrow back if the task is cancelled: the poster,
    /// or the funding charter's backer.
    pub refund_to: AccountId,
    /// A commitment to the task's specification, kept off the ledger: its
    /// input, its environment and grader, its licence.
    pub spec: Hash32,
    /// The rule a result must pass.
    pub acceptance: Acceptance,
    /// The worker, or `None` for an open task anyone may settle.
    pub worker: Option<AccountId>,
    /// The escrow, paid to the worker in full on acceptance.
    pub amount: Amount,
    /// The last Pulse a result may be settled.
    pub deadline: Pulse,
    /// When it was posted.
    pub posted_at: Pulse,
    /// Where it stands.
    pub state: TaskState,
}

/// The atom of `worker`'s task bond.
pub fn worker_bond(worker: &AccountId) -> AtomId {
    bond_atom(AtomClass::Economic, worker, WORKER_BOND_LABEL).id
}

/// The formula a result claim must carry (spec §5.43 step 5): a dual
/// signature, and the worker's task bond or any of the `attestations`, and
/// every required claim in ascending order. Every worker needs a task bond,
/// however small, to issue it: a claim's atoms must be registered.
pub fn result_formula(
    worker: &AccountId,
    attestations: &[ClaimId],
    requires: &BTreeSet<ClaimId>,
) -> Formula {
    let mut alternatives = vec![Formula::atom(worker_bond(worker))];
    alternatives.extend(attestations.iter().map(|a| Formula::claim(*a)));
    let mut parts = vec![formulas::dual_signature(), Formula::or(alternatives)];
    parts.extend(requires.iter().map(|r| Formula::claim(*r)));
    Formula::and(parts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sion_core::encode::decode;

    #[test]
    fn a_result_payload_decodes_to_its_task() {
        let result = TaskResult {
            task: ObjectId::derive(b"task"),
            output: Hash32::ZERO,
        };
        let Payload::Public(bytes) = result.payload() else {
            panic!("results are public");
        };
        assert_eq!(decode::<TaskResult>(&bytes).unwrap(), result);
        let mut longer = bytes.clone();
        longer.push(0);
        assert!(decode::<TaskResult>(&longer).is_err());
    }

    #[test]
    fn the_result_formula_rests_on_the_bond_or_an_attestation_and_every_requirement() {
        let worker = AccountId::derive(b"worker");
        let requires: BTreeSet<ClaimId> = [ClaimId::derive(b"environment")].into();
        let bonded = result_formula(&worker, &[], &requires);
        assert_eq!(
            bonded,
            Formula::and(vec![
                formulas::dual_signature(),
                Formula::atom(worker_bond(&worker)),
                Formula::claim(ClaimId::derive(b"environment")),
            ])
        );
        let attested = result_formula(&worker, &[ClaimId::derive(b"v1")], &requires);
        assert_eq!(attested.claim_refs().len(), 2);
        assert_eq!(attested.direct_atoms().len(), 3);
    }
}
