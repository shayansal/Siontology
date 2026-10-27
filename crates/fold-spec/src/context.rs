//! The Pulse context (spec §1): what every step of a chunk executes under.

use fold_core::hash::Hash32;
use fold_core::ids::Pulse;
use fold_exec::state::ChainConfig;

/// The context a chunk executes in: `ctx = { pulse: S.pulse, beacon:
/// S.beacon, config: W.config }`. Signatures are always verified (proof
/// stacks never skip them), so there is no switch for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpecContext {
    /// The Pulse being executed.
    pub pulse: Pulse,
    /// The Pulse's beacon.
    pub beacon: Hash32,
    /// The chain configuration.
    pub config: ChainConfig,
}
