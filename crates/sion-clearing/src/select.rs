//! Choosing among competing solver submissions.
//!
//! Anyone can run a solver. The ledger discards every submission whose
//! certificate fails and keeps the best certified objective; with the beacon
//! perturbation the optimum is unique, so in practice every surviving
//! submission is the same allocation and the earliest one wins.

use sion_lp::Q;

use crate::assignment::{verify_assignment, AssignmentBatch, AssignmentResult};
use crate::auction::{verify_clearing, Batch, ClearingResult};

/// Returns the index and objective of the best verifying candidate (lowest
/// index on ties), or `None` if none verifies.
pub fn select_best<S, E>(
    candidates: &[S],
    mut verify: impl FnMut(&S) -> Result<Q, E>,
) -> Option<(usize, Q)> {
    let mut best: Option<(usize, Q)> = None;
    for (index, candidate) in candidates.iter().enumerate() {
        let Ok(objective) = verify(candidate) else {
            continue;
        };
        let better = match &best {
            None => true,
            Some((_, current)) => objective > *current,
        };
        if better {
            best = Some((index, objective));
        }
    }
    best
}

/// The best verifying clearing submission for a batch.
pub fn select_best_clearing(batch: &Batch, candidates: &[ClearingResult]) -> Option<usize> {
    select_best(candidates, |candidate| {
        verify_clearing(batch, candidate).map(|verified| verified.objective)
    })
    .map(|(index, _)| index)
}

/// The best verifying assignment submission for a batch.
pub fn select_best_assignment(
    batch: &AssignmentBatch,
    candidates: &[AssignmentResult],
) -> Option<usize> {
    select_best(candidates, |candidate| {
        verify_assignment(batch, candidate).map(|_| candidate.objective.clone())
    })
    .map(|(index, _)| index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_only_verifying_candidates() {
        let candidates = [
            Err(()),
            Ok(Q::from(3u64)),
            Ok(Q::from(5u64)),
            Ok(Q::from(5u64)),
        ];
        let best = select_best(&candidates, |c| c.clone());
        assert_eq!(best, Some((2, Q::from(5u64))));
        let none: [Result<Q, ()>; 2] = [Err(()), Err(())];
        assert_eq!(select_best(&none, |c| c.clone()), None);
    }
}
