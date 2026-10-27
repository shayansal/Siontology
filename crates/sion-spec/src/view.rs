//! The objects a chunk executes against.
//!
//! A chunk sees the state before it only through its witness (spec §2.1,
//! §8.5): every read that is not served by a write made earlier in the
//! chunk must hit an identifier listed in the witness, or the whole chunk
//! is rejected. A listed identifier may say the object is absent; reading
//! it is then a read of a missing object, an ordinary operation error.

use std::collections::{BTreeMap, BTreeSet};

use sion_core::ids::ObjectId;
use sion_exec::object::Object;

use crate::error::SpecError;

/// The witnessed pre-state of a chunk plus the writes kept so far.
#[derive(Clone, Debug, Default)]
pub(crate) struct ChunkView {
    /// The witnessed objects (`None`: proven absent), as they were before
    /// the chunk.
    witnessed: BTreeMap<ObjectId, Option<Object>>,
    /// Writes of transactions (or of the deferred phase) that were kept.
    kept: BTreeMap<ObjectId, Object>,
    /// Identifiers whose reads were served by the witness.
    reads: BTreeSet<ObjectId>,
}

impl ChunkView {
    /// A view over verified witness entries.
    pub(crate) fn new(witnessed: BTreeMap<ObjectId, Option<Object>>) -> Self {
        ChunkView {
            witnessed,
            kept: BTreeMap::new(),
            reads: BTreeSet::new(),
        }
    }

    /// Reads `id` as the chunk currently sees it. Reads that fall through to
    /// the pre-state are recorded in `reads`; an identifier outside the
    /// witness rejects the chunk.
    pub(crate) fn get(
        &self,
        id: &ObjectId,
        reads: &mut BTreeSet<ObjectId>,
    ) -> Result<Option<Object>, SpecError> {
        if let Some(object) = self.kept.get(id) {
            return Ok(Some(object.clone()));
        }
        match self.witnessed.get(id) {
            Some(entry) => {
                reads.insert(*id);
                Ok(entry.clone())
            }
            None => Err(SpecError::Unwitnessed(*id)),
        }
    }

    /// Keeps a finished step's writes.
    pub(crate) fn keep(&mut self, writes: BTreeMap<ObjectId, Object>) {
        self.kept.extend(writes);
    }

    /// Records reads made by a step, whether or not its writes are kept
    /// (a discarded run still read what it read).
    pub(crate) fn note_reads(&mut self, reads: BTreeSet<ObjectId>) {
        self.reads.extend(reads);
    }

    /// Splits the view into its kept writes and its reads.
    pub(crate) fn into_parts(self) -> (BTreeMap<ObjectId, Object>, BTreeSet<ObjectId>) {
        (self.kept, self.reads)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sion_core::ids::{AccountId, AssetId, Version};
    use sion_core::Amount;
    use sion_exec::object::{Coin, ObjectData, Owner};

    fn coin(tag: &[u8], amount: u128) -> Object {
        Object {
            id: ObjectId::derive(tag),
            version: Version(3),
            owner: Owner::Account(AccountId::derive(b"o")),
            data: ObjectData::Coin(Coin {
                asset: AssetId::usd(),
                amount: Amount(amount),
            }),
        }
    }

    #[test]
    fn reads_fall_through_to_the_witness_and_stop_at_its_edge() {
        let present = coin(b"a", 5);
        let absent = ObjectId::derive(b"b");
        let mut witnessed = BTreeMap::new();
        witnessed.insert(present.id, Some(present.clone()));
        witnessed.insert(absent, None);
        let mut view = ChunkView::new(witnessed);
        let mut reads = BTreeSet::new();
        assert_eq!(
            view.get(&present.id, &mut reads).unwrap(),
            Some(present.clone())
        );
        assert_eq!(view.get(&absent, &mut reads).unwrap(), None);
        let outside = ObjectId::derive(b"c");
        assert_eq!(
            view.get(&outside, &mut reads),
            Err(SpecError::Unwitnessed(outside))
        );
        assert_eq!(reads.len(), 2);

        // A kept write shadows the witness and needs no witness entry.
        let created = coin(b"c", 9);
        view.keep([(created.id, created.clone())].into_iter().collect());
        let mut more = BTreeSet::new();
        assert_eq!(view.get(&outside, &mut more).unwrap(), Some(created));
        assert!(more.is_empty());
        view.note_reads(reads);
        let (kept, reads) = view.into_parts();
        assert_eq!(reads.len(), 2);
        assert_eq!(kept.len(), 1);
    }
}
