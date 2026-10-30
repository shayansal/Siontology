//! Cohorts on this machine (spec v1.14): what `sion agent cohort`
//! committed to, kept so that `sion agent materialize` can prove its
//! members later.
//!
//! A cohort made here has identical keyless members, so its file holds the
//! member once with the count, and the tree is rebuilt when a proof is
//! needed: about two million hashes for a full cohort of 2²⁰.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sion_core::hash::Hash32;
use sion_core::ids::{AccountId, ObjectId};
use sion_core::Amount;
use sion_exec::charter::lineage_id;
use sion_exec::cohort::{CohortMember, CohortTree};

use crate::error::CliError;

/// A cohort made on this machine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    /// The cohort, once created.
    pub cohort: Option<ObjectId>,
    /// The parent charter.
    pub parent: ObjectId,
    /// The lineage index of member 0.
    pub first: u64,
    /// How many members.
    pub count: u32,
    /// Each member's budget.
    pub budget: Amount,
    /// Each member's bucket capacity.
    pub burst: Amount,
    /// Each member's refill per Pulse.
    pub rate: Amount,
    /// Each member's card commitment.
    pub card: Hash32,
    /// Materialized members are named `<prefix>-<index>`.
    pub prefix: String,
    /// The wallet that signs for the parent, and so for the members.
    pub signer: String,
}

impl Plan {
    /// The member every leaf holds.
    pub fn member(&self) -> CohortMember {
        CohortMember {
            agent: None,
            card: self.card,
            budget: self.budget,
            burst: self.burst,
            rate: self.rate,
        }
    }

    /// The sum tree over the members.
    pub fn tree(&self) -> Result<CohortTree, CliError> {
        let members = vec![self.member(); self.count as usize];
        CohortTree::build(&members)
            .ok_or_else(|| CliError::Usage("the members' sums overflow".into()))
    }

    /// The cohort's id.
    pub fn cohort_id(&self) -> Result<ObjectId, CliError> {
        self.cohort
            .ok_or_else(|| CliError::Usage("the cohort was not created".into()))
    }

    /// Member `index`'s agent id.
    pub fn member_id(&self, index: u32) -> ObjectId {
        lineage_id(&self.parent, self.first + u64::from(index))
    }

    /// Member `index`'s account.
    pub fn member_account(&self, index: u32) -> AccountId {
        sion_exec::charter::keyless_account(&self.member_id(index))
    }
}

/// Where cohorts are kept: `$SION_HOME/agents/cohorts`.
pub fn dir() -> PathBuf {
    crate::session::wallet_dir()
        .parent()
        .map(|home| home.join("agents").join("cohorts"))
        .unwrap_or_else(|| PathBuf::from(".sion/agents/cohorts"))
}

/// Saves a created cohort as `<cohort id>.json`.
pub fn save(plan: &Plan) -> Result<PathBuf, CliError> {
    let id = plan.cohort_id()?;
    let dir = dir();
    std::fs::create_dir_all(&dir)
        .map_err(|e| CliError::Usage(format!("{}: {e}", dir.display())))?;
    let path = dir.join(format!("{id}.json"));
    let text = serde_json::to_string_pretty(plan).map_err(|e| CliError::Usage(e.to_string()))?;
    std::fs::write(&path, text).map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?;
    Ok(path)
}

/// The cohort whose id is `text` or starts with it.
pub fn find(text: &str) -> Result<Plan, CliError> {
    let text = text.trim().to_lowercase();
    let mut found = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir()) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(id) = name.strip_suffix(".json") {
                if id.starts_with(&text) {
                    found.push(entry.path());
                }
            }
        }
    }
    match found.as_slice() {
        [path] => {
            let text = std::fs::read_to_string(path)
                .map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?;
            serde_json::from_str(&text)
                .map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))
        }
        [] => Err(CliError::Usage(format!(
            "no cohort {text} in {}",
            dir().display()
        ))),
        _ => Err(CliError::Usage(format!(
            "{text} names more than one cohort"
        ))),
    }
}
