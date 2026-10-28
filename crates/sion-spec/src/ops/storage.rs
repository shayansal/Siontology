//! Storage deals: PostDeal (§5.50), AcceptDeal (§5.51), ProveStorage
//! (§5.52), SettleDeal (§5.53) and CloseDeal (§5.54).
//!
//! PLACEHOLDER: stack B does not implement specification v1.12 yet. Every
//! storage operation fails here, so the differential tests report the
//! difference until it is written from the specification.

use crate::error::{fail, Step};
use crate::tx::TxRun;

/// Any storage operation, until v1.12 is written in this stack.
pub(super) fn not_yet(_run: &mut TxRun<'_>) -> Step {
    Err(fail(
        "storage deals (v1.12) are not implemented in proof stack B yet",
    ))
}
