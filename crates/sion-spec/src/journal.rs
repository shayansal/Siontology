//! Scratch bookkeeping shared by transactions and the deferred phase
//! (spec §3.1): `load`, `try_load`, `store` and `consume` over a scratch
//! view whose writes are kept only if the caller decides so.

use std::collections::{BTreeMap, BTreeSet};

use sion_core::ids::{ObjectId, Pulse, TxDigest, Version};
use sion_exec::object::{Object, ObjectData, Tombstone};

use crate::error::{fail, Step};
use crate::view::ChunkView;

/// The scratch state of one step (one transaction run, or the deferred
/// phase): its own writes over the chunk view, plus the bookkeeping that
/// assigns versions and builds the created / mutated / consumed sets.
#[derive(Debug)]
pub(crate) struct Scratch<'v> {
    /// What earlier steps of the chunk left behind.
    view: &'v ChunkView,
    /// This step's writes.
    writes: BTreeMap<ObjectId, Object>,
    /// `originals`: the version of every pre-existing object when this step
    /// first read it.
    originals: BTreeMap<ObjectId, Version>,
    /// Objects this step created (`created`, which is also `effects.created`).
    created: BTreeSet<ObjectId>,
    /// `effects.mutated`.
    mutated: BTreeSet<ObjectId>,
    /// `effects.consumed`.
    consumed: BTreeSet<ObjectId>,
    /// Identifiers this step read from the witness.
    reads: BTreeSet<ObjectId>,
}

/// What a finished step leaves behind.
#[derive(Debug, Default)]
pub(crate) struct ScratchParts {
    /// The step's writes.
    pub writes: BTreeMap<ObjectId, Object>,
    /// Objects created.
    pub created: BTreeSet<ObjectId>,
    /// Objects mutated.
    pub mutated: BTreeSet<ObjectId>,
    /// Objects consumed.
    pub consumed: BTreeSet<ObjectId>,
    /// Identifiers read from the witness.
    pub reads: BTreeSet<ObjectId>,
}

impl<'v> Scratch<'v> {
    /// Fresh bookkeeping over `view`.
    pub(crate) fn new(view: &'v ChunkView) -> Self {
        Scratch {
            view,
            writes: BTreeMap::new(),
            originals: BTreeMap::new(),
            created: BTreeSet::new(),
            mutated: BTreeSet::new(),
            consumed: BTreeSet::new(),
            reads: BTreeSet::new(),
        }
    }

    /// The object as this step sees it: its own writes, then the chunk view.
    fn current(&mut self, id: &ObjectId) -> Step<Option<Object>> {
        if let Some(object) = self.writes.get(id) {
            return Ok(Some(object.clone()));
        }
        Ok(self.view.get(id, &mut self.reads)?)
    }

    /// **try_load(id)**: the object, or nothing if it does not exist. Records
    /// the version of a pre-existing object the first time it is read.
    pub(crate) fn try_load(&mut self, id: &ObjectId) -> Step<Option<Object>> {
        let object = self.current(id)?;
        if let Some(found) = &object {
            if !self.created.contains(id) {
                self.originals.entry(*id).or_insert(found.version);
            }
        }
        Ok(object)
    }

    /// **load(id)**: like [`Self::try_load`], but a missing object is an
    /// error ("missing object").
    pub(crate) fn load(&mut self, id: &ObjectId) -> Step<Object> {
        match self.try_load(id)? {
            Some(object) => Ok(object),
            None => Err(fail(format!("missing object {id}"))),
        }
    }

    /// **store(object)**: writes `object`, assigning its version: 0 for an
    /// object this step created; one more than the original for a
    /// pre-existing object (which then counts as mutated unless it is a
    /// tombstone); otherwise 0, and the object counts as created.
    pub(crate) fn store(&mut self, mut object: Object) -> Step {
        let id = object.id;
        if self.created.contains(&id) {
            object.version = Version(0);
        } else if let Some(original) = self.originals.get(&id) {
            let next = original
                .0
                .checked_add(1)
                .ok_or_else(|| fail("version overflow"))?;
            object.version = Version(next);
            if !object.is_tombstone() {
                self.mutated.insert(id);
            }
        } else {
            object.version = Version(0);
            self.created.insert(id);
        }
        self.writes.insert(id, object);
        Ok(())
    }

    /// **consume(id)**: replaces the object with a tombstone that keeps its
    /// identifier, owner and (through `store`) its version line; the object
    /// counts as consumed rather than mutated. Returns the object as loaded.
    pub(crate) fn consume(&mut self, id: &ObjectId, by: TxDigest, at: Pulse) -> Step<Object> {
        let object = self.load(id)?;
        self.store(Object {
            id: *id,
            version: object.version,
            owner: object.owner,
            data: ObjectData::Tombstone(Tombstone {
                consumed_by: by,
                at,
            }),
        })?;
        self.mutated.remove(id);
        self.consumed.insert(*id);
        Ok(object)
    }

    /// Ends the step, handing back its writes and bookkeeping.
    pub(crate) fn finish(self) -> ScratchParts {
        ScratchParts {
            writes: self.writes,
            created: self.created,
            mutated: self.mutated,
            consumed: self.consumed,
            reads: self.reads,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sion_core::ids::{AccountId, AssetId};
    use sion_core::Amount;
    use sion_exec::object::{Coin, Owner};

    fn coin(tag: &[u8], version: u64) -> Object {
        Object {
            id: ObjectId::derive(tag),
            version: Version(version),
            owner: Owner::Account(AccountId::derive(b"o")),
            data: ObjectData::Coin(Coin {
                asset: AssetId::usd(),
                amount: Amount(10),
            }),
        }
    }

    fn view_with(objects: &[Object], absent: &[ObjectId]) -> ChunkView {
        let mut witnessed: BTreeMap<ObjectId, Option<Object>> =
            objects.iter().map(|o| (o.id, Some(o.clone()))).collect();
        for id in absent {
            witnessed.insert(*id, None);
        }
        ChunkView::new(witnessed)
    }

    #[test]
    fn versions_follow_the_bookkeeping_rules() {
        let existing = coin(b"a", 4);
        let fresh = ObjectId::derive(b"new");
        let view = view_with(std::slice::from_ref(&existing), &[fresh]);
        let mut scratch = Scratch::new(&view);

        // A pre-existing object: one more than its original version, mutated.
        let mut loaded = scratch.load(&existing.id).unwrap();
        loaded.version = Version(99);
        scratch.store(loaded.clone()).unwrap();
        scratch.store(loaded).unwrap();
        // Storing twice still bumps once relative to the original.
        assert_eq!(scratch.load(&existing.id).unwrap().version, Version(5));

        // A missing object is an error; storing it creates it at version 0.
        assert!(scratch.load(&fresh).is_err());
        assert_eq!(scratch.try_load(&fresh).unwrap(), None);
        let mut created = coin(b"new", 7);
        created.id = fresh;
        scratch.store(created.clone()).unwrap();
        assert_eq!(scratch.load(&fresh).unwrap().version, Version(0));
        scratch.store(created).unwrap();
        assert_eq!(scratch.load(&fresh).unwrap().version, Version(0));

        let parts = scratch.finish();
        assert_eq!(parts.created, [fresh].into_iter().collect());
        assert_eq!(parts.mutated, [existing.id].into_iter().collect());
        assert!(parts.consumed.is_empty());
        assert_eq!(parts.reads.len(), 2);
    }

    #[test]
    fn consume_tombstones_and_moves_from_mutated_to_consumed() {
        let existing = coin(b"a", 2);
        let view = view_with(std::slice::from_ref(&existing), &[]);
        let mut scratch = Scratch::new(&view);
        let mut loaded = scratch.load(&existing.id).unwrap();
        scratch.store(loaded.clone()).unwrap();
        loaded = scratch
            .consume(&existing.id, TxDigest::derive(b"t"), Pulse(8))
            .unwrap();
        assert_eq!(loaded.version, Version(3), "the object as loaded");
        let tombstone = scratch.load(&existing.id).unwrap();
        assert!(tombstone.is_tombstone());
        assert_eq!(tombstone.version, Version(3));
        assert_eq!(tombstone.owner, existing.owner);
        let parts = scratch.finish();
        assert!(parts.mutated.is_empty());
        assert_eq!(parts.consumed, [existing.id].into_iter().collect());
    }

    #[test]
    fn reads_outside_the_witness_reject_the_chunk() {
        let view = view_with(&[], &[]);
        let mut scratch = Scratch::new(&view);
        let outside = ObjectId::derive(b"z");
        assert!(matches!(
            scratch.try_load(&outside),
            Err(crate::error::Abort::Chunk(
                crate::error::SpecError::Unwitnessed(_)
            ))
        ));
    }
}
