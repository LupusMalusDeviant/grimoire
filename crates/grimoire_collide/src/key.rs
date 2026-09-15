//! Stable collider identity across ECS entities and pool objects (contract §14).

use grimoire_core::impl_stable_hash;
use grimoire_ecs::Entity;

/// Colliders that belong to a [`grimoire_ecs::World`] entity.
pub const SOURCE_ENTITY: u8 = 0;

/// Colliders that live in a pool without an entity (for example Sigil bullets by `BulletId`,
/// contract §11.3). Reserved for the facade adapter (contract §9.6).
///
/// `2..=127` are reserved for engine crates, `128..=255` for games.
pub const SOURCE_POOL: u8 = 1;

/// Identity of one collider, unique within one [`crate::SpatialGrid::rebuild`] call.
///
/// Ordering compares `source`, then `index`, then `generation` — for entities this matches
/// [`Entity`] ordering (index before generation); for pool objects it matches `BulletId` ordering.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ColliderKey {
    /// Which kind of object this key names: [`SOURCE_ENTITY`], [`SOURCE_POOL`], or a
    /// crate-/game-reserved source.
    pub source: u8,
    /// Slot index within its source.
    pub index: u32,
    /// Generation of the slot, distinguishing a reused slot from the object it replaced.
    pub generation: u32,
}
impl_stable_hash!(ColliderKey {
    source,
    index,
    generation
});

impl ColliderKey {
    /// Key for a world entity.
    #[must_use]
    pub fn from_entity(entity: Entity) -> Self {
        Self {
            source: SOURCE_ENTITY,
            index: entity.index(),
            generation: entity.generation(),
        }
    }

    /// The entity this key names, or `None` if it was not built from [`SOURCE_ENTITY`].
    #[must_use]
    pub fn entity(self) -> Option<Entity> {
        (self.source == SOURCE_ENTITY)
            .then(|| Entity::from_bits((u64::from(self.generation) << 32) | u64::from(self.index)))
    }

    /// Key for a pool object without an entity ([`SOURCE_POOL`]).
    #[must_use]
    pub const fn pool(index: u32, generation: u32) -> Self {
        Self {
            source: SOURCE_POOL,
            index,
            generation,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_entity_and_back_round_trips() {
        let entity = Entity::from_bits((7u64 << 32) | 3);
        let key = ColliderKey::from_entity(entity);
        assert_eq!(key.source, SOURCE_ENTITY);
        assert_eq!(key.index, 3);
        assert_eq!(key.generation, 7);
        assert_eq!(key.entity(), Some(entity));
    }

    #[test]
    fn pool_key_has_no_entity() {
        let key = ColliderKey::pool(5, 2);
        assert_eq!(key.source, SOURCE_POOL);
        assert_eq!(key.entity(), None);
    }

    #[test]
    fn ordering_is_source_then_index_then_generation() {
        let a = ColliderKey {
            source: 0,
            index: 5,
            generation: 9,
        };
        let b = ColliderKey {
            source: 0,
            index: 6,
            generation: 0,
        };
        let c = ColliderKey {
            source: 1,
            index: 0,
            generation: 0,
        };
        assert!(a < b);
        assert!(b < c);
        let d = ColliderKey {
            source: 0,
            index: 5,
            generation: 10,
        };
        assert!(a < d);
    }
}
