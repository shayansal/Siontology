//! Safety and liveness of Contention Consensus under the simulator.

use std::collections::BTreeSet;

use fold_consensus::{Behavior, CoreConfig, LeaderSchedule, SimConfig, SimReport, Simulation};
use fold_core::hash_domain;
use proptest::prelude::*;

fn assert_safe_and_live(report: &SimReport) {
    report.check_safety().expect("safety");
    report.check_liveness().expect("liveness");
    assert!(
        report.evidence_verifies(),
        "every reported evidence verifies"
    );
    assert!(
        report.offenders().is_subset(&report.faulty),
        "evidence never incriminates an honest validator"
    );
}

#[test]
fn four_honest_validators_commit_every_item() {
    let report = Simulation::run_config(SimConfig::new(4, 1));
    assert_safe_and_live(&report);
    for validator in &report.honest {
        assert!(report.commit_count(validator) > 30, "steady commits");
    }
    let delivered: usize = report
        .commits
        .values()
        .next()
        .unwrap()
        .iter()
        .map(|c| c.items.len())
        .sum();
    assert_eq!(
        delivered,
        report.submitted.len(),
        "every item delivered exactly once"
    );
}

#[test]
fn seven_validators_tolerate_two_crashes() {
    let config = SimConfig::new(7, 3)
        .with_behavior(1, Behavior::Crash { at_ms: 0 })
        .with_behavior(4, Behavior::Crash { at_ms: 3_000 });
    assert_safe_and_live(&Simulation::run_config(config));
}

#[test]
fn adversarial_network_before_gst_then_live() {
    let config = SimConfig::new(7, 4).with_gst(3_000, 200);
    let report = Simulation::run_config(config);
    assert!(
        report.dropped_messages > 0,
        "the network really dropped messages"
    );
    assert_safe_and_live(&report);
}

#[test]
fn an_equivocating_proposer_cannot_break_safety() {
    let config = SimConfig::new(4, 5)
        .with_behavior(2, Behavior::EquivocatingProposer { double_vote: false });
    assert_safe_and_live(&Simulation::run_config(config));
}

#[test]
fn f_colluding_double_voting_equivocators_cannot_break_safety() {
    // n = 7 tolerates f = 2: two colluders cannot gather two quorums.
    let config = SimConfig::new(7, 6)
        .with_behavior(0, Behavior::EquivocatingProposer { double_vote: true })
        .with_behavior(3, Behavior::EquivocatingProposer { double_vote: true });
    let report = Simulation::run_config(config);
    assert_safe_and_live(&report);
}

#[test]
fn silent_voters_and_invalid_signatures_are_tolerated() {
    let config = SimConfig::new(7, 7)
        .with_behavior(0, Behavior::SilentVoter)
        .with_behavior(5, Behavior::InvalidSignatures);
    assert_safe_and_live(&Simulation::run_config(config));
}

#[test]
fn stake_weighted_committee_and_leaders() {
    let mut config = SimConfig::new(4, 8)
        .with_stakes(vec![4, 3, 2, 1])
        .with_behavior(3, Behavior::Crash { at_ms: 0 });
    config.core = CoreConfig {
        leader_schedule: LeaderSchedule::StakeWeighted {
            seed: hash_domain("test/beacon", b"epoch-0"),
        },
        ..CoreConfig::default()
    };
    assert_safe_and_live(&Simulation::run_config(config));
}

/// With f + 1 colluding, double-voting equivocators (2 of 4) the fault
/// assumption is broken. What the protocol guarantees then is
/// accountability: conflicting certificates are detected and the evidence
/// names only the colluders. In these runs safety also held, because honest
/// validators refuse a certificate that conflicts with one they hold; they
/// stall instead, so liveness is lost.
#[test]
fn beyond_the_fault_bound_conflicts_are_detected_and_attributed() {
    for seed in 11..15 {
        let config = SimConfig::new(4, seed)
            .with_behavior(1, Behavior::EquivocatingProposer { double_vote: true })
            .with_behavior(3, Behavior::EquivocatingProposer { double_vote: true });
        let report = Simulation::run_config(config);
        assert!(
            !report.evidence.is_empty(),
            "conflicting certificates were detected"
        );
        assert!(report.evidence_verifies());
        assert_eq!(
            report.offenders(),
            report.faulty,
            "exactly the colluders are named"
        );
        if report.check_safety().is_err() {
            assert!(
                report.offenders().len() >= 2,
                "divergence is always attributable"
            );
        }
    }
}

#[test]
fn garbage_collection_bounds_the_dag() {
    let mut config = SimConfig::new(4, 9);
    config.core.gc_depth = 10;
    config.duration_ms = 10_000;
    let report = Simulation::run_config(config);
    assert_safe_and_live(&report);
    for (validator, rounds) in &report.dag_rounds {
        assert!(
            *rounds <= 16,
            "validator {} holds {rounds} rounds",
            validator.short()
        );
    }
}

#[test]
fn simulation_is_deterministic() {
    let run = || Simulation::run_config(SimConfig::new(4, 21).with_gst(1_000, 100));
    let a = run();
    let b = run();
    assert_eq!(a.commits, b.commits);
    assert_eq!(a.delivered_messages, b.delivered_messages);
}

fn behavior_strategy() -> impl Strategy<Value = Behavior> {
    prop_oneof![
        (0u64..6_000).prop_map(|at_ms| Behavior::Crash { at_ms }),
        any::<bool>().prop_map(|double_vote| Behavior::EquivocatingProposer { double_vote }),
        Just(Behavior::SilentVoter),
        Just(Behavior::InvalidSignatures),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(16))]
    #[test]
    fn random_faults_within_tolerance_preserve_safety_and_liveness(
        seed in 0u64..1_000_000,
        seven in any::<bool>(),
        gst in prop_oneof![Just(0u64), Just(2_000u64)],
        drop in prop_oneof![Just(0u32), Just(150u32)],
        behaviors in proptest::collection::vec(behavior_strategy(), 2),
        picks in proptest::collection::vec(0usize..7, 2),
    ) {
        let n = if seven { 7 } else { 4 };
        let f = (n - 1) / 3;
        let mut config = SimConfig::new(n, seed).with_gst(gst, drop);
        let mut chosen = BTreeSet::new();
        for (behavior, pick) in behaviors.into_iter().zip(picks) {
            if chosen.len() == f {
                break;
            }
            let index = pick % n;
            if chosen.insert(index) {
                config = config.with_behavior(index, behavior);
            }
        }
        let report = Simulation::run_config(config);
        prop_assert!(report.check_safety().is_ok(), "{:?}", report.check_safety());
        prop_assert!(report.check_liveness().is_ok(), "{:?}", report.check_liveness());
        prop_assert!(report.evidence_verifies());
        prop_assert!(report.offenders().is_subset(&report.faulty));
    }
}
