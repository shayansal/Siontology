//! Assignment-market tests: random instances against brute force, tamper
//! rejection, and individually rational settlement.

use fold_clearing::{
    select_best_assignment, settle_assignment, solve_assignment, verify_assignment, Assignment,
    AssignmentBatch, Bid, ClearingError, Job, Slot,
};
use fold_core::{hash_domain, AccountId, Amount, DomainId, Hash32, Pulse};
use fold_lp::Q;
use proptest::prelude::*;

fn id(tag: &str) -> Hash32 {
    hash_domain("test/id", tag.as_bytes())
}

/// Builds a batch from a value matrix (`None` = no bid) and reserves, in
/// whole units.
fn batch(values: &[Vec<Option<u64>>], reserves: &[u64], seed: u64) -> AssignmentBatch {
    AssignmentBatch {
        domain: DomainId::derive(b"compute-slots"),
        pulse: Pulse(9),
        beacon: hash_domain("test/beacon", &seed.to_be_bytes()),
        slots: reserves
            .iter()
            .enumerate()
            .map(|(s, reserve)| Slot {
                id: id(&format!("slot{s}")),
                owner: AccountId::derive(format!("owner{s}").as_bytes()),
                reserve: Amount::units(*reserve),
            })
            .collect(),
        jobs: values
            .iter()
            .enumerate()
            .map(|(j, row)| Job {
                id: id(&format!("job{j}")),
                owner: AccountId::derive(format!("job-owner{j}").as_bytes()),
                bids: row
                    .iter()
                    .enumerate()
                    .filter_map(|(s, value)| {
                        value.map(|v| Bid {
                            slot: id(&format!("slot{s}")),
                            value: Amount::units(v),
                        })
                    })
                    .collect(),
            })
            .collect(),
    }
}

/// Best total surplus over all matchings, by exhaustive search.
fn brute_force(values: &[Vec<Option<u64>>], reserves: &[u64]) -> i128 {
    fn search(
        job: usize,
        values: &[Vec<Option<u64>>],
        reserves: &[u64],
        used: &mut Vec<bool>,
    ) -> i128 {
        if job == values.len() {
            return 0;
        }
        let mut best = search(job + 1, values, reserves, used);
        for (s, value) in values[job].iter().enumerate() {
            if let Some(v) = value {
                if !used[s] {
                    used[s] = true;
                    let gain = i128::from(*v) - i128::from(reserves[s]);
                    best = best.max(gain + search(job + 1, values, reserves, used));
                    used[s] = false;
                }
            }
        }
        best
    }
    search(0, values, reserves, &mut vec![false; reserves.len()])
}

#[test]
fn tampered_premiums_and_matchings_are_rejected() {
    let values = vec![vec![Some(10), Some(8)], vec![Some(9), Some(3)]];
    let b = batch(&values, &[0, 0], 1);
    let honest = solve_assignment(&b).unwrap();
    verify_assignment(&b, &honest).unwrap();

    let mut premiums = honest.clone();
    premiums.slot_premiums[0] = &premiums.slot_premiums[0] + &Q::one();
    assert!(matches!(
        verify_assignment(&b, &premiums),
        Err(ClearingError::PriceMismatch)
    ));

    let mut matching = honest.clone();
    matching.assignments = vec![
        Assignment {
            job: id("job0"),
            slot: id("slot0"),
        },
        Assignment {
            job: id("job1"),
            slot: id("slot1"),
        },
    ];
    assert!(matches!(
        verify_assignment(&b, &matching),
        Err(ClearingError::FillMismatch(_))
    ));

    // The worse matching with a certificate built for it does not verify.
    let mut worse = honest.clone();
    worse.certificate.x = vec![Q::one(), Q::zero(), Q::zero(), Q::one()];
    worse.assignments = matching.assignments.clone();
    assert!(verify_assignment(&b, &worse).is_err());

    assert_eq!(select_best_assignment(&b, &[worse, honest]), Some(1));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Random instances: the certified matching is 0/1, verifies, matches
    /// the brute-force optimum, and settles without any job paying more
    /// than its value or any slot receiving less than its reserve.
    #[test]
    fn random_assignments_are_optimal_and_rational(
        jobs in 1usize..=4,
        slots in 1usize..=4,
        raw in proptest::collection::vec(proptest::option::of(0u64..=12), 16),
        reserves_raw in proptest::collection::vec(0u64..=6, 4),
        seed in 0u64..1_000,
    ) {
        let values: Vec<Vec<Option<u64>>> = (0..jobs)
            .map(|j| (0..slots).map(|s| raw[j * 4 + s]).collect())
            .collect();
        let reserves: Vec<u64> = reserves_raw[..slots].to_vec();
        let b = batch(&values, &reserves, seed);
        let result = solve_assignment(&b).unwrap();
        let welfare = verify_assignment(&b, &result).unwrap();
        prop_assert_eq!(i128::try_from(welfare.micros() / 1_000_000).unwrap(), brute_force(&values, &reserves));
        prop_assert_eq!(welfare.micros() % 1_000_000, 0);

        let transfers = settle_assignment(&b, &result).unwrap();
        prop_assert_eq!(transfers.len(), result.assignments.len());
        for (assignment, transfer) in result.assignments.iter().zip(&transfers) {
            let job = b.jobs.iter().find(|j| j.id == assignment.job).unwrap();
            let slot = b.slots.iter().find(|s| s.id == assignment.slot).unwrap();
            let value = job.bids.iter().find(|bid| bid.slot == slot.id).unwrap().value;
            prop_assert!(transfer.amount <= value, "job pays at most its value");
            prop_assert!(transfer.amount >= slot.reserve, "slot gets at least its reserve");
        }
    }
}
