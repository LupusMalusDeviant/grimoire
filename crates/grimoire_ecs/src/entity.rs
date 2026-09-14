//! Entity handles and the generational entity allocator.

use std::collections::VecDeque;
use std::fmt;

use grimoire_core::{StableHash, StableHasher};

/// Generational handle to an entity of a [`World`](crate::World).
///
/// An entity is identified by a slot `index` and a `generation`. Despawning an entity bumps the
/// generation of its slot, so handles to despawned entities stay invalid even after the slot has
/// been reused.
///
/// Ordering compares the index first and the generation second.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Entity {
    index: u32,
    generation: u32,
}

impl Entity {
    /// Slot index of the entity inside its world.
    #[must_use]
    pub const fn index(self) -> u32 {
        self.index
    }

    /// Generation of the slot at the time the entity was spawned.
    #[must_use]
    pub const fn generation(self) -> u32 {
        self.generation
    }

    /// Packs the handle into 64 bits: generation in the upper, index in the lower half.
    #[must_use]
    pub const fn to_bits(self) -> u64 {
        ((self.generation as u64) << 32) | self.index as u64
    }

    /// Inverse of [`Entity::to_bits`].
    #[must_use]
    pub const fn from_bits(bits: u64) -> Self {
        Self {
            index: bits as u32,
            generation: (bits >> 32) as u32,
        }
    }

    pub(crate) const fn new(index: u32, generation: u32) -> Self {
        Self { index, generation }
    }
}

impl fmt::Debug for Entity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Entity({}v{})", self.index, self.generation)
    }
}

impl fmt::Display for Entity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}v{}", self.index, self.generation)
    }
}

impl StableHash for Entity {
    fn stable_hash(&self, hasher: &mut StableHasher) {
        hasher.write_u64(self.to_bits());
    }
}

/// Position of a live entity inside the archetype storage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EntityLocation {
    pub(crate) archetype: usize,
    pub(crate) row: usize,
}

#[derive(Clone, Debug)]
struct EntityMeta {
    generation: u32,
    location: Option<EntityLocation>,
}

/// Generational allocator with a FIFO free list.
///
/// Reuse order: freed slots are reused oldest-first (the slot despawned earliest is handed out
/// by the next spawn). New slots are only appended when the free list is empty. A slot whose
/// generation would overflow `u32::MAX` is retired and never reused, so a stale handle can never
/// alias a live entity.
#[derive(Clone, Debug, Default)]
pub(crate) struct Entities {
    metas: Vec<EntityMeta>,
    free: VecDeque<u32>,
    alive: usize,
}

impl Entities {
    /// Allocates a handle for an entity stored at `location`.
    pub(crate) fn alloc(&mut self, location: EntityLocation) -> Entity {
        self.alive += 1;
        if let Some(index) = self.free.pop_front() {
            let meta = &mut self.metas[index as usize];
            meta.location = Some(location);
            return Entity::new(index, meta.generation);
        }
        let Ok(index) = u32::try_from(self.metas.len()) else {
            panic!("entity index space exhausted: more than u32::MAX entity slots");
        };
        self.metas.push(EntityMeta {
            generation: 0,
            location: Some(location),
        });
        Entity::new(index, 0)
    }

    /// Releases a live entity and returns where it was stored.
    pub(crate) fn free(&mut self, entity: Entity) -> Option<EntityLocation> {
        let meta = self.metas.get_mut(entity.index as usize)?;
        if meta.generation != entity.generation {
            return None;
        }
        let location = meta.location.take()?;
        self.alive -= 1;
        if let Some(next) = meta.generation.checked_add(1) {
            meta.generation = next;
            self.free.push_back(entity.index);
        }
        Some(location)
    }

    /// Location of a live entity, `None` for despawned or stale handles.
    pub(crate) fn location(&self, entity: Entity) -> Option<EntityLocation> {
        let meta = self.metas.get(entity.index as usize)?;
        if meta.generation == entity.generation {
            meta.location
        } else {
            None
        }
    }

    /// Updates the location of a live entity after its row moved.
    pub(crate) fn set_location(&mut self, entity: Entity, location: EntityLocation) {
        if let Some(meta) = self.metas.get_mut(entity.index as usize) {
            debug_assert_eq!(meta.generation, entity.generation, "stale location update");
            meta.location = Some(location);
        }
    }

    /// Number of live entities.
    pub(crate) const fn alive(&self) -> usize {
        self.alive
    }

    /// Feeds slot generations, liveness and the free list in order.
    pub(crate) fn stable_hash(&self, hasher: &mut StableHasher) {
        hasher.write_usize(self.metas.len());
        for meta in &self.metas {
            hasher.write_u32(meta.generation);
            hasher.write_bool(meta.location.is_some());
        }
        hasher.write_usize(self.free.len());
        for &index in &self.free {
            hasher.write_u32(index);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOC: EntityLocation = EntityLocation {
        archetype: 0,
        row: 0,
    };

    #[test]
    fn bits_round_trip() {
        let entity = Entity::new(7, 3);
        assert_eq!(entity.to_bits(), (3 << 32) | 7);
        assert_eq!(Entity::from_bits(entity.to_bits()), entity);
        assert_eq!(entity.index(), 7);
        assert_eq!(entity.generation(), 3);
    }

    #[test]
    fn ordering_is_index_first() {
        assert!(Entity::new(1, 9) < Entity::new(2, 0));
        assert!(Entity::new(1, 0) < Entity::new(1, 1));
    }

    #[test]
    fn debug_and_display_format() {
        assert_eq!(format!("{:?}", Entity::new(4, 2)), "Entity(4v2)");
        assert_eq!(Entity::new(4, 2).to_string(), "4v2");
    }

    #[test]
    fn free_list_is_fifo_and_bumps_generation() {
        let mut entities = Entities::default();
        let a = entities.alloc(LOC);
        let b = entities.alloc(LOC);
        let c = entities.alloc(LOC);
        assert_eq!(entities.alive(), 3);
        assert!(entities.free(b).is_some());
        assert!(entities.free(a).is_some());
        assert!(entities.free(a).is_none(), "double free must fail");
        let first = entities.alloc(LOC);
        let second = entities.alloc(LOC);
        assert_eq!(first, Entity::new(b.index(), 1));
        assert_eq!(second, Entity::new(a.index(), 1));
        assert_eq!(entities.alloc(LOC), Entity::new(3, 0));
        assert!(entities.location(a).is_none());
        assert!(entities.location(c).is_some());
    }

    #[test]
    fn exhausted_generation_retires_slot() {
        let mut entities = Entities::default();
        let a = entities.alloc(LOC);
        entities.metas[a.index() as usize].generation = u32::MAX;
        let a = Entity::new(a.index(), u32::MAX);
        assert!(entities.free(a).is_some());
        assert!(entities.free.is_empty());
        assert_eq!(entities.alloc(LOC).index(), 1);
    }

    #[test]
    fn unknown_index_is_not_alive() {
        let entities = Entities::default();
        assert!(entities.location(Entity::new(99, 0)).is_none());
    }
}
