//! The claim index: IndexClaim (§5.58).
//!
//! Written from the specification. It calls `sion_exec::index::index_id`,
//! which only derives an id; the key's rule and the forward-only rule are
//! checked here.

use sion_assurance::ClaimStatus;
use sion_core::ids::{ClaimId, ObjectId, Version};
use sion_exec::execute::Event;
use sion_exec::index::{index_id, IndexEntry};
use sion_exec::object::{Object, ObjectData, Owner};
use sion_exec::tx::object_domain;

use crate::error::{fail, Step};
use crate::ops::assurance::load_assurance;
use crate::tx::TxRun;

/// The longest key, in bytes (§5.58).
const MAX_KEY_BYTES: usize = 64;

/// §5.58 step 1: 1 to 64 bytes, each from `0x21` to `0x7e`.
fn key_allowed(key: &str) -> bool {
    let bytes = key.as_bytes();
    !bytes.is_empty() && bytes.len() <= MAX_KEY_BYTES && bytes.iter().all(|&b| b > 0x20 && b < 0x7f)
}

/// §5.58 IndexClaim.
pub(super) fn index_claim(run: &mut TxRun<'_>, key: &str, claim: ClaimId) -> Step {
    let pulse = run.ctx.pulse;
    let sender = run.sender();
    // Step 1.
    if !key_allowed(key) {
        return Err(fail(
            "the key is not 1 to 64 printable characters without spaces",
        ));
    }
    // Step 2.
    let loaded = load_assurance(run)?;
    let Some(record) = loaded.domain.registry.claim(&claim) else {
        return Err(fail("the claim is unknown"));
    };
    if record.status != ClaimStatus::Active || record.issuer != sender {
        return Err(fail("the claim is not an active claim of the sender's"));
    }
    let sequence = record.sequence;
    // Step 3.
    let id: ObjectId = index_id(&sender, key);
    let entry = match run.try_load(&id)? {
        Some(object) => {
            let ObjectData::IndexEntry(found) = &object.data else {
                return Err(fail("the id holds something other than an index entry"));
            };
            let earlier = loaded
                .domain
                .registry
                .claim(&found.claim)
                .map(|r| r.sequence);
            match earlier {
                Some(e) if e < sequence => {}
                _ => return Err(fail("the entry names a claim that is not earlier")),
            }
            let entry = IndexEntry {
                owner: found.owner,
                key: found.key.clone(),
                claim,
                previous: Some(found.claim),
                count: found.count.saturating_add(1),
                updated_at: pulse,
            };
            run.store(Object {
                data: ObjectData::IndexEntry(entry.clone()),
                ..object
            })?;
            entry
        }
        None => {
            let entry = IndexEntry {
                owner: sender,
                key: key.to_string(),
                claim,
                previous: None,
                count: 1,
                updated_at: pulse,
            };
            run.store(Object {
                id,
                version: Version(0),
                owner: Owner::Shared(object_domain("index", &id)),
                data: ObjectData::IndexEntry(entry.clone()),
            })?;
            entry
        }
    };
    // Step 4.
    run.event(Event::ClaimIndexed {
        entry: id,
        owner: sender,
        key: key.to_string(),
        claim,
        count: entry.count,
    });
    Ok(())
}
