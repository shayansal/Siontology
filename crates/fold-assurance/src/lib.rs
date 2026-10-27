//! The Assurance Algebra.
//!
//! Every claim carries a monotone Boolean formula over the assumptions it
//! rests on (its *atoms*). From that formula any consumer computes, under its
//! own policy, the minimum an attacker must spend or forfeit to make the claim
//! false: the claim's **cost of corruption**. An agent accepts a claim only
//! when that cost exceeds what it has at stake, and reserves the claim's
//! backing so total reliance can never exceed what slashing can repay.
//!
//! Modules:
//!
//! - [`atom`]: atoms and their classes, plus the standard atoms (hash
//!   functions, lattices, proof stacks, the specification).
//! - [`formula`]: trust formulas (`∧`, `∨`, atoms and references to other
//!   claims), normalization and evaluation.
//! - [`policy`]: a consumer's policy: which atoms it trusts and its safety
//!   factor κ.
//! - [`registry`]: claims, bonds, reservations and broken atoms.
//! - [`coc`]: the cost-of-corruption interval (`min` over `∧`, `+` over
//!   atom-disjoint `∨`, `max` as a sound lower bound when atoms are shared)
//!   and an exact solver for formulas with few shared atoms.
//! - [`reserve`]: pushing a reservation of κ·V down a formula: in full on
//!   every `∧` input, split across `∨` alternatives, never beyond a bond.
//! - [`decide`]: the decision rule
//!   `min(δ·CoC_low, CoC(adjudicator)) ≥ κ·V`.
//! - [`challenge`]: challenges, rulings, slashing, falsification propagation
//!   through dependent claims (truth maintenance) and compensation.
//! - [`insurance`]: failure probabilities for independent atoms.

pub mod atom;
pub mod challenge;
pub mod coc;
pub mod decide;
pub mod error;
pub mod formula;
pub mod insurance;
pub mod policy;
pub mod registry;
pub mod reserve;

pub use atom::{Atom, AtomClass};
pub use challenge::{Challenge, ChallengeStatus, Compensation, SlashOutcome};
pub use coc::{coc_interval, exact_coc, CocInterval};
pub use decide::{accept_and_reserve, decide, release, Decision};
pub use error::AssuranceError;
pub use formula::Formula;
pub use policy::{Policy, Ratio};
pub use registry::{BondState, ClaimRecord, ClaimStatus, Registry, Reservation, ReservationStatus};
pub use reserve::{allocate, max_reservable, Allocation};
