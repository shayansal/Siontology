//! Stress: many random fault configurations within tolerance.
use fold_consensus::{Behavior, SimConfig, Simulation};
use rand_core::RngCore;

fn main() {
    let mut rng = fold_core::rng::seeded(2026);
    let mut failures = 0;
    let runs = 300;
    for run in 0..runs {
        let n = if rng.next_u32() % 2 == 0 { 4 } else { 7 };
        let f = (n - 1) / 3;
        let gst = if rng.next_u32() % 2 == 0 { 0 } else { 2_500 };
        let drop = [0u32, 100, 250][(rng.next_u32() % 3) as usize];
        let seed = rng.next_u64();
        let mut config = SimConfig::new(n, seed).with_gst(gst, drop);
        let mut used = std::collections::BTreeSet::new();
        while used.len() < f {
            let index = (rng.next_u32() as usize) % n;
            if !used.insert(index) {
                continue;
            }
            let behavior = match rng.next_u32() % 5 {
                0 => Behavior::Crash {
                    at_ms: rng.next_u64() % 8_000,
                },
                1 => Behavior::EquivocatingProposer { double_vote: false },
                2 => Behavior::EquivocatingProposer { double_vote: true },
                3 => Behavior::SilentVoter,
                _ => Behavior::InvalidSignatures,
            };
            config = config.with_behavior(index, behavior);
        }
        let behaviors = format!("{:?}", config.behaviors);
        let report = Simulation::run_config(config);
        let safety = report.check_safety();
        let liveness = report.check_liveness();
        let blame = report.offenders().is_subset(&report.faulty) && report.evidence_verifies();
        if safety.is_err() || liveness.is_err() || !blame {
            failures += 1;
            println!("run {run} n={n} seed={seed} gst={gst} drop={drop} {behaviors}: safety {safety:?} liveness {liveness:?} blame_ok {blame}");
        }
    }
    println!("{failures} failures out of {runs}");
}
