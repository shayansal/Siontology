//! Assignment markets: matching jobs to slots (compute slots, delivery
//! windows, rides) with a certificate that no better matching exists.
//!
//! ```text
//! maximize   Σ (value_js − reserve_s + δ·π_js) · x_js
//! subject to Σ_s x_js ≤ 1   for every job j     (rows 0 .. J)
//!            Σ_j x_js ≤ 1   for every slot s    (rows J .. J + S)
//!            x ≥ 0
//! ```
//!
//! The constraint matrix is the incidence matrix of a bipartite graph, which
//! is totally unimodular, so every vertex — and therefore the optimum the
//! simplex returns — is a 0/1 matching. The beacon perturbation makes the
//! optimal matching unique, so the allocation is canonical.
//!
//! The slot duals are prices (premiums over each slot's reserve) that form a
//! competitive equilibrium: every job prefers its assigned slot at those
//! prices. They are one certified equilibrium price vector, not a canonical
//! one; several price vectors can support the same matching.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use fold_core::{hash_parts, AccountId, Amount, DomainId, Hash32, Pulse};
use fold_lp::{is_integral, solve, verify_certificate, Certificate, LinearProgram, LpOutcome, Q};
use num_bigint::BigInt;
use num_traits::ToPrimitive;
use serde::{Deserialize, Serialize};

use crate::auction::Transfer;
use crate::error::ClearingError;
use crate::priority::{delta, priority};

/// Something that can be assigned to at most one job.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Slot {
    /// Unique within the batch.
    pub id: Hash32,
    /// Who is paid for the slot.
    pub owner: AccountId,
    /// The minimum the owner accepts, in micro-units.
    pub reserve: Amount,
}

/// A job's value for one slot.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Bid {
    /// The slot.
    pub slot: Hash32,
    /// What the job is willing to pay for it, in micro-units.
    pub value: Amount,
}

/// Something that needs at most one slot.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Job {
    /// Unique within the batch.
    pub id: Hash32,
    /// Who pays.
    pub owner: AccountId,
    /// Values for acceptable slots.
    pub bids: Vec<Bid>,
}

/// One sealed batch of an assignment market.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct AssignmentBatch {
    /// The market.
    pub domain: DomainId,
    /// The Pulse the batch clears in.
    pub pulse: Pulse,
    /// Randomness revealed after sealing.
    pub beacon: Hash32,
    /// Jobs in canonical order.
    pub jobs: Vec<Job>,
    /// Slots in canonical order.
    pub slots: Vec<Slot>,
}

/// A job matched to a slot.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub struct Assignment {
    /// The job.
    pub job: Hash32,
    /// The slot.
    pub slot: Hash32,
}

/// A solver's submission for an assignment batch.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct AssignmentResult {
    /// Matches in pair order (job order, then bid order).
    pub assignments: Vec<Assignment>,
    /// Certified price premium over reserve for each slot, in slot order.
    pub slot_premiums: Vec<Q>,
    /// Optimality certificate for the perturbed program.
    pub certificate: Certificate,
    /// The certified (perturbed) objective.
    pub objective: Q,
    /// Gains from trade at the original values: Σ (value − reserve).
    pub welfare: Amount,
}

/// One candidate job-slot pair, i.e. one program variable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pair {
    /// Index of the job in the batch.
    pub job: usize,
    /// Index of the slot in the batch.
    pub slot: usize,
    /// The job's value for the slot.
    pub value: Amount,
}

/// Enumerates the program's variables: one per (job, bid) in job order then
/// bid order. Rejects duplicate IDs, unknown slots and duplicate bids.
pub fn pairs(batch: &AssignmentBatch) -> Result<Vec<Pair>, ClearingError> {
    let mut slot_index = BTreeMap::new();
    for (index, slot) in batch.slots.iter().enumerate() {
        if slot_index.insert(slot.id, index).is_some() {
            return Err(ClearingError::InvalidBatch("duplicate slot id".into()));
        }
    }
    let mut job_ids = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for (job_index, job) in batch.jobs.iter().enumerate() {
        if !job_ids.insert(job.id) {
            return Err(ClearingError::InvalidBatch("duplicate job id".into()));
        }
        let mut seen = std::collections::BTreeSet::new();
        for bid in &job.bids {
            let Some(&slot) = slot_index.get(&bid.slot) else {
                return Err(ClearingError::InvalidBatch(format!(
                    "job {} bids on unknown slot {}",
                    job.id, bid.slot
                )));
            };
            if !seen.insert(slot) {
                return Err(ClearingError::InvalidBatch(format!(
                    "job {} bids twice on slot {}",
                    job.id, bid.slot
                )));
            }
            out.push(Pair {
                job: job_index,
                slot,
                value: bid.value,
            });
        }
    }
    Ok(out)
}

/// Builds the assignment program deterministically from the batch.
pub fn build_assignment_program(
    batch: &AssignmentBatch,
) -> Result<(LinearProgram, Vec<Pair>), ClearingError> {
    let pairs = pairs(batch)?;
    let scale = delta(pairs.len() as u128);
    let mut program = LinearProgram::new(pairs.len());
    for (var, pair) in pairs.iter().enumerate() {
        let job = &batch.jobs[pair.job];
        let slot = &batch.slots[pair.slot];
        let pair_id = hash_parts("fold/clearing/pair", &[&job.id.0, &slot.id.0]);
        let surplus = Q::from(pair.value.micros()) - Q::from(slot.reserve.micros());
        program.set_objective(var, surplus + &scale * &priority(&batch.beacon, &pair_id));
    }
    for job_index in 0..batch.jobs.len() {
        let terms: Vec<(usize, Q)> = pairs
            .iter()
            .enumerate()
            .filter(|(_, pair)| pair.job == job_index)
            .map(|(var, _)| (var, Q::one()))
            .collect();
        program.add_le(&terms, Q::one());
    }
    for slot_index in 0..batch.slots.len() {
        let terms: Vec<(usize, Q)> = pairs
            .iter()
            .enumerate()
            .filter(|(_, pair)| pair.slot == slot_index)
            .map(|(var, _)| (var, Q::one()))
            .collect();
        program.add_le(&terms, Q::one());
    }
    Ok((program, pairs))
}

fn welfare_of(batch: &AssignmentBatch, pairs: &[Pair], x: &[Q]) -> Result<Amount, ClearingError> {
    let mut total = BigInt::from(0);
    for (pair, value) in pairs.iter().zip(x) {
        if value.is_positive() {
            total += BigInt::from(pair.value.micros())
                - BigInt::from(batch.slots[pair.slot].reserve.micros());
        }
    }
    total.to_u128().map(Amount).ok_or(ClearingError::Overflow)
}

/// Solves an assignment batch with the reference simplex.
pub fn solve_assignment(batch: &AssignmentBatch) -> Result<AssignmentResult, ClearingError> {
    let (program, pairs) = build_assignment_program(batch)?;
    let solution = match solve(&program).map_err(|e| ClearingError::Solver(e.to_string()))? {
        LpOutcome::Optimal(solution) => solution,
        other => {
            return Err(ClearingError::Solver(format!(
                "assignment program is always feasible and bounded, got {other:?}"
            )))
        }
    };
    if !is_integral(&solution.x) {
        return Err(ClearingError::NotIntegral);
    }
    let assignments = pairs
        .iter()
        .zip(&solution.x)
        .filter(|(_, value)| value.is_positive())
        .map(|(pair, _)| Assignment {
            job: batch.jobs[pair.job].id,
            slot: batch.slots[pair.slot].id,
        })
        .collect();
    let job_rows = batch.jobs.len();
    let slot_premiums = solution.y[job_rows..].to_vec();
    let welfare = welfare_of(batch, &pairs, &solution.x)?;
    Ok(AssignmentResult {
        assignments,
        slot_premiums,
        certificate: solution.certificate(),
        objective: solution.objective,
        welfare,
    })
}

/// Verifies an assignment submission: the program is rebuilt from the batch,
/// the certificate passes with zero tolerance, the matching is 0/1 and is
/// exactly the certificate's, the premiums are the certified slot duals, and
/// objective and welfare are recomputed.
pub fn verify_assignment(
    batch: &AssignmentBatch,
    result: &AssignmentResult,
) -> Result<Amount, ClearingError> {
    let (program, pairs) = build_assignment_program(batch)?;
    let report = verify_certificate(&program, &result.certificate, &Q::zero())?;
    if report.primal_objective != result.objective {
        return Err(ClearingError::ObjectiveMismatch {
            claimed: Box::new(result.objective.clone()),
            certified: Box::new(report.primal_objective),
        });
    }
    let x = &result.certificate.x;
    if x.iter()
        .any(|value| !(value.is_zero() || *value == Q::one()))
    {
        return Err(ClearingError::NotIntegral);
    }
    let expected: Vec<Assignment> = pairs
        .iter()
        .zip(x)
        .filter(|(_, value)| value.is_positive())
        .map(|(pair, _)| Assignment {
            job: batch.jobs[pair.job].id,
            slot: batch.slots[pair.slot].id,
        })
        .collect();
    if expected != result.assignments {
        return Err(ClearingError::FillMismatch(
            "assignments differ from the certificate".into(),
        ));
    }
    if result.certificate.y[batch.jobs.len()..] != result.slot_premiums[..] {
        return Err(ClearingError::PriceMismatch);
    }
    let welfare = welfare_of(batch, &pairs, x)?;
    if welfare != result.welfare {
        return Err(ClearingError::WelfareMismatch {
            expected: welfare,
            got: result.welfare,
        });
    }
    Ok(welfare)
}

/// Payments for a verified assignment: each job owner pays the slot owner
/// the reserve plus the certified premium, rounded down to a micro-unit.
/// Complementary slackness makes the premium at most the job's surplus, so
/// no job pays more than its value.
pub fn settle_assignment(
    batch: &AssignmentBatch,
    result: &AssignmentResult,
) -> Result<Vec<Transfer>, ClearingError> {
    verify_assignment(batch, result)?;
    let slot_index: BTreeMap<Hash32, usize> = batch
        .slots
        .iter()
        .enumerate()
        .map(|(index, slot)| (slot.id, index))
        .collect();
    let job_owner: BTreeMap<Hash32, AccountId> =
        batch.jobs.iter().map(|job| (job.id, job.owner)).collect();
    let mut transfers = Vec::with_capacity(result.assignments.len());
    for assignment in &result.assignments {
        let index = slot_index[&assignment.slot];
        let slot = &batch.slots[index];
        let premium = result.slot_premiums[index]
            .floor_u128()
            .ok_or(ClearingError::Overflow)?;
        let amount = slot
            .reserve
            .micros()
            .checked_add(premium)
            .map(Amount)
            .ok_or(ClearingError::Overflow)?;
        transfers.push(Transfer {
            from: job_owner[&assignment.job],
            to: slot.owner,
            amount,
        });
    }
    Ok(transfers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fold_core::hash_domain;

    fn id(tag: &str) -> Hash32 {
        hash_domain("test/id", tag.as_bytes())
    }

    #[test]
    fn two_by_two_prefers_the_better_matching() {
        // Job A values slot 1 at 10 and slot 2 at 8; job B values slot 1 at 9
        // and slot 2 at 3. Best: A→2, B→1 (welfare 17) beats A→1, B→2 (13).
        let batch = AssignmentBatch {
            domain: DomainId::derive(b"slots"),
            pulse: Pulse(3),
            beacon: id("beacon"),
            slots: vec![
                Slot {
                    id: id("s1"),
                    owner: AccountId::derive(b"o1"),
                    reserve: Amount::ZERO,
                },
                Slot {
                    id: id("s2"),
                    owner: AccountId::derive(b"o2"),
                    reserve: Amount::ZERO,
                },
            ],
            jobs: vec![
                Job {
                    id: id("a"),
                    owner: AccountId::derive(b"a"),
                    bids: vec![
                        Bid {
                            slot: id("s1"),
                            value: Amount::units(10),
                        },
                        Bid {
                            slot: id("s2"),
                            value: Amount::units(8),
                        },
                    ],
                },
                Job {
                    id: id("b"),
                    owner: AccountId::derive(b"b"),
                    bids: vec![
                        Bid {
                            slot: id("s1"),
                            value: Amount::units(9),
                        },
                        Bid {
                            slot: id("s2"),
                            value: Amount::units(3),
                        },
                    ],
                },
            ],
        };
        let result = solve_assignment(&batch).unwrap();
        assert_eq!(result.welfare, Amount::units(17));
        assert!(result.assignments.contains(&Assignment {
            job: id("a"),
            slot: id("s2")
        }));
        assert!(result.assignments.contains(&Assignment {
            job: id("b"),
            slot: id("s1")
        }));
        verify_assignment(&batch, &result).unwrap();
        let transfers = settle_assignment(&batch, &result).unwrap();
        assert_eq!(transfers.len(), 2);
    }

    #[test]
    fn unknown_slot_is_rejected() {
        let batch = AssignmentBatch {
            domain: DomainId::derive(b"slots"),
            pulse: Pulse(1),
            beacon: id("beacon"),
            slots: vec![],
            jobs: vec![Job {
                id: id("a"),
                owner: AccountId::derive(b"a"),
                bids: vec![Bid {
                    slot: id("nope"),
                    value: Amount::units(1),
                }],
            }],
        };
        assert!(matches!(
            solve_assignment(&batch),
            Err(ClearingError::InvalidBatch(_))
        ));
    }
}
