//! Prints commit statistics for a few simulated scenarios.
//!
//! Run with `cargo run --release -p sion-consensus --example simulate`.

use sion_consensus::{Behavior, SimConfig, Simulation};

fn show(name: &str, config: SimConfig) {
    let report = Simulation::run_config(config);
    let commits: Vec<usize> = report
        .honest
        .iter()
        .map(|v| report.commit_count(v))
        .collect();
    println!(
        "{name:<40} commits {commits:?} max round {} messages {} dropped {} safe {} live {} evidence {}",
        report.max_round.0,
        report.delivered_messages,
        report.dropped_messages,
        report.check_safety().is_ok(),
        report.check_liveness().is_ok(),
        report.evidence.len(),
    );
}

fn main() {
    show("4 honest validators", SimConfig::new(4, 1));
    show(
        "7 validators, 2 crashes",
        SimConfig::new(7, 3)
            .with_behavior(1, Behavior::Crash { at_ms: 0 })
            .with_behavior(4, Behavior::Crash { at_ms: 3_000 }),
    );
    show(
        "7 validators, GST 3 s, 20% drops",
        SimConfig::new(7, 4).with_gst(3_000, 200),
    );
    show(
        "4 validators, 1 equivocator",
        SimConfig::new(4, 5)
            .with_behavior(2, Behavior::EquivocatingProposer { double_vote: false }),
    );
    show(
        "7 validators, silent + invalid signatures",
        SimConfig::new(7, 6)
            .with_behavior(0, Behavior::SilentVoter)
            .with_behavior(5, Behavior::InvalidSignatures),
    );
    show(
        "4 validators, 2 colluders (beyond f)",
        SimConfig::new(4, 7)
            .with_behavior(1, Behavior::EquivocatingProposer { double_vote: true })
            .with_behavior(3, Behavior::EquivocatingProposer { double_vote: true }),
    );
}
