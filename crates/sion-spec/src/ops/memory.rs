//! Memory logs: OpenMemory (§5.40) and CommitMemory (§5.41).
//!
//! Written from the specification. The log's head moves only along a
//! consistency proof, checked with the shared Merkle primitive
//! (`sion_core::merkle::verify_consistency`, RFC 9162); the charter chain
//! check of an agent's log is §5.13's, implemented in this crate.

use sion_core::hash::Hash32;
use sion_core::ids::ObjectId;
use sion_core::merkle;
use sion_exec::execute::Event;
use sion_exec::memory::{MemoryLog, MAX_CONSISTENCY_PROOF};
use sion_exec::object::{ObjectData, Owner};

use crate::error::{fail, Step};
use crate::ops::charter::{as_charter, load_identity, standing};
use crate::tx::TxRun;

/// §5.40 OpenMemory.
pub(super) fn open(run: &mut TxRun<'_>, agent: Option<ObjectId>, policy: Hash32) -> Step {
    let sender = run.sender();
    // Step 1.
    if let Some(agent_id) = agent {
        let charter = load_identity(run, agent_id)?;
        if charter.agent != sender {
            return Err(fail("only the agent opens its own memory log"));
        }
        standing(run, agent_id, &charter)?;
    }
    // Step 2.
    let pulse = run.ctx.pulse;
    let log = run.create(
        Owner::Account(sender),
        ObjectData::MemoryLog(MemoryLog {
            writer: sender,
            agent,
            policy,
            size: 0,
            root: merkle::empty_root(),
            index: Hash32::ZERO,
            manifest: Hash32::ZERO,
            commits: 0,
            opened_at: pulse,
            committed_at: pulse,
            sealed: false,
        }),
    )?;
    // Step 3.
    run.event(Event::MemoryOpened {
        log,
        account: sender,
        agent,
    });
    Ok(())
}

/// The new head a commit names (§5.41).
pub(super) struct NewHead<'a> {
    pub(super) size: u64,
    pub(super) root: Hash32,
    pub(super) index: Hash32,
    pub(super) manifest: Hash32,
    pub(super) proof: &'a [Hash32],
    pub(super) seal: bool,
}

/// §5.41 CommitMemory.
pub(super) fn commit(run: &mut TxRun<'_>, head: NewHead<'_>) -> Step {
    // Step 1.
    let log_id = run
        .tx
        .inputs
        .get(1)
        .map(|input| input.id)
        .ok_or_else(|| fail("the memory log must be the second input"))?;
    let mut object = run.load(&log_id)?;
    let mut log = match &object.data {
        ObjectData::MemoryLog(log) => log.clone(),
        _ => return Err(fail(format!("{log_id} is not a memory log"))),
    };
    if log.writer != run.sender() {
        return Err(fail("only the log's writer commits"));
    }
    if log.sealed {
        return Err(fail("the log is sealed"));
    }
    // Step 2.
    if let Some(agent) = log.agent {
        let charter = as_charter(&run.load(&agent)?)?;
        standing(run, agent, &charter)?;
    }
    // Step 3.
    if head.proof.len() > MAX_CONSISTENCY_PROOF {
        return Err(fail("the consistency proof is too long"));
    }
    if head.size < log.size {
        return Err(fail("the new size is below the log's"));
    }
    if head.size == 0 && head.root != merkle::empty_root() {
        return Err(fail("a log of no atoms must have the empty root"));
    }
    if merkle::verify_consistency(&log.root, log.size, &head.root, head.size, head.proof).is_err() {
        return Err(fail("the consistency proof does not verify"));
    }
    // Step 4.
    log.size = head.size;
    log.root = head.root;
    log.index = head.index;
    log.manifest = head.manifest;
    log.commits = log.commits.saturating_add(1);
    log.committed_at = run.ctx.pulse;
    if head.seal {
        log.sealed = true;
    }
    let commits = log.commits;
    let sealed = log.sealed;
    object.data = ObjectData::MemoryLog(log);
    run.store(object)?;
    // Step 5.
    run.event(Event::MemoryCommitted {
        log: log_id,
        size: head.size,
        root: head.root,
        commits,
        sealed,
    });
    Ok(())
}
