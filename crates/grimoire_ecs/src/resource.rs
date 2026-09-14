//! World-global singletons ("resources") in registration order.

use std::any::{Any, TypeId};
use std::collections::BTreeMap;

use grimoire_core::{StableHash, StableHasher};

/// World-global singleton data, such as the simulation tick or input of the current step.
///
/// Blanket-implemented for every `'static + Send + Sync + Clone + StableHash` type.
pub trait Resource: 'static + Send + Sync + Clone + StableHash {}

impl<T: 'static + Send + Sync + Clone + StableHash> Resource for T {}

trait ResourceStorage: Any + Send + Sync + 'static {
    fn clone_storage(&self) -> Box<dyn ResourceStorage>;
    fn hash_value(&self, hasher: &mut StableHasher);
}

impl<R: Resource> ResourceStorage for R {
    fn clone_storage(&self) -> Box<dyn ResourceStorage> {
        Box::new(self.clone())
    }

    fn hash_value(&self, hasher: &mut StableHasher) {
        self.stable_hash(hasher);
    }
}

/// Resource slots in first-insertion order. Removing a resource keeps its slot, so re-inserting
/// it later restores the original position.
#[derive(Default)]
pub(crate) struct Resources {
    ids: BTreeMap<TypeId, usize>,
    slots: Vec<Option<Box<dyn ResourceStorage>>>,
}

impl Clone for Resources {
    fn clone(&self) -> Self {
        Self {
            ids: self.ids.clone(),
            slots: self
                .slots
                .iter()
                .map(|slot| slot.as_deref().map(ResourceStorage::clone_storage))
                .collect(),
        }
    }
}

impl Resources {
    pub(crate) fn insert<R: Resource>(&mut self, resource: R) {
        let boxed: Box<dyn ResourceStorage> = Box::new(resource);
        match self.ids.get(&TypeId::of::<R>()) {
            Some(&index) => self.slots[index] = Some(boxed),
            None => {
                self.ids.insert(TypeId::of::<R>(), self.slots.len());
                self.slots.push(Some(boxed));
            }
        }
    }

    pub(crate) fn get<R: Resource>(&self) -> Option<&R> {
        let index = *self.ids.get(&TypeId::of::<R>())?;
        let storage: &dyn ResourceStorage = self.slots[index].as_deref()?;
        let any: &dyn Any = storage;
        any.downcast_ref::<R>()
    }

    pub(crate) fn get_mut<R: Resource>(&mut self) -> Option<&mut R> {
        let index = *self.ids.get(&TypeId::of::<R>())?;
        let storage: &mut dyn ResourceStorage = self.slots[index].as_deref_mut()?;
        let any: &mut dyn Any = storage;
        any.downcast_mut::<R>()
    }

    pub(crate) fn remove<R: Resource>(&mut self) -> Option<R> {
        let index = *self.ids.get(&TypeId::of::<R>())?;
        let storage = self.slots[index].take()?;
        let any: Box<dyn Any> = storage;
        any.downcast::<R>().ok().map(|boxed| *boxed)
    }

    /// Feeds the slot count and, per slot in registration order, a presence tag and the value.
    pub(crate) fn stable_hash(&self, hasher: &mut StableHasher) {
        hasher.write_usize(self.slots.len());
        for slot in &self.slots {
            match slot {
                None => hasher.write_u8(0),
                Some(value) => {
                    hasher.write_u8(1);
                    value.hash_value(hasher);
                }
            }
        }
    }
}
