//! Object stores and the execution overlay.
//!
//! Execution never mutates a base store directly. It runs over an
//! [`Overlay`] that records every object read and written, so the touched
//! set (and therefore the witness a prover needs) falls out of execution
//! for free. Each transaction runs inside a journaled scope that is
//! committed on success and rolled back on failure.

use std::collections::{BTreeMap, BTreeSet};

use sion_core::ids::ObjectId;

use crate::error::ExecError;
use crate::object::Object;

/// Read access to objects.
pub trait ObjectStore {
    /// Looks up an object. `Err` means the store cannot answer (for
    /// example, a witness that does not cover the key); `Ok(None)` means
    /// the object does not exist.
    fn get_object(&self, id: &ObjectId) -> Result<Option<Object>, ExecError>;
}

impl ObjectStore for BTreeMap<ObjectId, Object> {
    fn get_object(&self, id: &ObjectId) -> Result<Option<Object>, ExecError> {
        Ok(self.get(id).cloned())
    }
}

/// A recording, journaled overlay over a base store.
#[derive(Debug)]
pub struct Overlay<'a, S: ObjectStore> {
    base: &'a S,
    writes: BTreeMap<ObjectId, Object>,
    reads: BTreeSet<ObjectId>,
    journal: Option<Vec<(ObjectId, Option<Object>)>>,
}

impl<'a, S: ObjectStore> Overlay<'a, S> {
    /// A fresh overlay.
    pub fn new(base: &'a S) -> Self {
        Overlay {
            base,
            writes: BTreeMap::new(),
            reads: BTreeSet::new(),
            journal: None,
        }
    }

    /// The base store (the Pulse-start snapshot).
    pub fn base(&self) -> &'a S {
        self.base
    }

    /// Reads through the overlay.
    pub fn get(&mut self, id: &ObjectId) -> Result<Option<Object>, ExecError> {
        if let Some(object) = self.writes.get(id) {
            return Ok(Some(object.clone()));
        }
        self.reads.insert(*id);
        self.base.get_object(id)
    }

    /// Reads, failing if absent.
    pub fn require(&mut self, id: &ObjectId) -> Result<Object, ExecError> {
        self.get(id)?.ok_or(ExecError::MissingObject(*id))
    }

    /// Reads the Pulse-start version, ignoring writes in this Pulse.
    pub fn snapshot(&mut self, id: &ObjectId) -> Result<Option<Object>, ExecError> {
        self.reads.insert(*id);
        self.base.get_object(id)
    }

    /// Writes an object.
    pub fn put(&mut self, object: Object) {
        let id = object.id;
        if let Some(journal) = &mut self.journal {
            let previous = self.writes.get(&id).cloned();
            journal.push((id, previous));
        }
        self.writes.insert(id, object);
    }

    /// Starts a transaction scope.
    pub fn begin(&mut self) {
        debug_assert!(self.journal.is_none(), "scopes do not nest");
        self.journal = Some(Vec::new());
    }

    /// Keeps the scope's writes.
    pub fn commit(&mut self) {
        self.journal = None;
    }

    /// Undoes the scope's writes.
    pub fn rollback(&mut self) {
        if let Some(journal) = self.journal.take() {
            for (id, previous) in journal.into_iter().rev() {
                match previous {
                    Some(object) => {
                        self.writes.insert(id, object);
                    }
                    None => {
                        self.writes.remove(&id);
                    }
                }
            }
        }
    }

    /// Every object read or written.
    pub fn touched(&self) -> BTreeSet<ObjectId> {
        self.reads
            .iter()
            .chain(self.writes.keys())
            .copied()
            .collect()
    }

    /// The writes, consuming the overlay.
    pub fn into_writes(self) -> BTreeMap<ObjectId, Object> {
        self.writes
    }

    /// The writes so far.
    pub fn writes(&self) -> &BTreeMap<ObjectId, Object> {
        &self.writes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::{Coin, ObjectData, Owner};
    use sion_core::ids::{AccountId, AssetId, Version};
    use sion_core::Amount;

    fn coin(n: u8) -> Object {
        Object {
            id: ObjectId::derive(&[n]),
            version: Version(0),
            owner: Owner::Account(AccountId::derive(b"a")),
            data: ObjectData::Coin(Coin {
                asset: AssetId::usd(),
                amount: Amount::units(u64::from(n)),
            }),
        }
    }

    #[test]
    fn rollback_restores_previous_writes() {
        let base: BTreeMap<ObjectId, Object> = [(coin(1).id, coin(1))].into_iter().collect();
        let mut overlay = Overlay::new(&base);
        let mut first = coin(1);
        first.version = Version(1);
        overlay.put(first.clone());
        overlay.begin();
        let mut second = coin(1);
        second.version = Version(2);
        overlay.put(second);
        overlay.put(coin(2));
        overlay.rollback();
        assert_eq!(
            overlay.get(&coin(1).id).unwrap().unwrap().version,
            Version(1)
        );
        assert!(overlay.get(&coin(2).id).unwrap().is_none());
        assert_eq!(
            overlay.snapshot(&coin(1).id).unwrap().unwrap().version,
            Version(0)
        );
        assert!(overlay.touched().contains(&coin(2).id));
    }
}
