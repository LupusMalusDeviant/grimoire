//! The [`World`]: entities, archetype storage, resources, hashing and snapshots.

use std::collections::BTreeMap;
use std::fmt;

use grimoire_core::{StableHash, StableHasher};

use crate::archetype::Archetype;
use crate::bundle::Bundle;
use crate::component::{Component, ComponentId, ComponentRegistry};
use crate::entity::{Entities, Entity, EntityLocation};
use crate::error::EcsError;
use crate::query::{Query, QueryIter, QueryIterMut, ReadOnlyQuery};
use crate::resource::{Resource, Resources};

/// Archetype index of the component-less archetype, created by [`World::new`].
const EMPTY_ARCHETYPE: usize = 0;

/// Container of all entities, components and resources of one simulation.
///
/// All state changes are deterministic functions of the call sequence: component ids follow
/// registration order, archetypes are created lazily in first-use order, rows are dense and
/// despawns swap-remove.
pub struct World {
    entities: Entities,
    components: ComponentRegistry,
    archetypes: Vec<Archetype>,
    archetype_index: BTreeMap<Box<[ComponentId]>, usize>,
    resources: Resources,
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for World {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("World")
            .field("entities", &self.entities.alive())
            .field("component_types", &self.components.count())
            .field("archetypes", &self.archetypes.len())
            .finish_non_exhaustive()
    }
}

impl World {
    /// Creates an empty world.
    #[must_use]
    pub fn new() -> Self {
        let mut world = Self {
            entities: Entities::default(),
            components: ComponentRegistry::default(),
            archetypes: Vec::new(),
            archetype_index: BTreeMap::new(),
            resources: Resources::default(),
        };
        let empty = world.archetype_for(&[]);
        debug_assert_eq!(empty, EMPTY_ARCHETYPE);
        world
    }

    /// Registers component type `C`. Idempotent; ids are assigned in registration order.
    ///
    /// Registration also happens implicitly on the first `spawn`/`insert` of `C` (in tuple
    /// order for bundles). Explicit registration pins ids independently of spawn order.
    pub fn register_component<C: Component>(&mut self) {
        self.components.register::<C>();
    }

    /// Spawns an entity with all components of `bundle`.
    ///
    /// Reuses despawned slots oldest-first before growing the entity table.
    ///
    /// # Panics
    ///
    /// If `bundle` contains the same component type more than once.
    pub fn spawn<B: Bundle>(&mut self, bundle: B) -> Entity {
        let ids = B::register(&mut self.components);
        let mut sorted = ids;
        sorted.as_mut().sort_unstable();
        if sorted.as_ref().windows(2).any(|pair| pair[0] == pair[1]) {
            panic!(
                "bundle `{}` contains the same component type more than once",
                std::any::type_name::<B>()
            );
        }
        let archetype_index = self.archetype_for(sorted.as_ref());
        let archetype = &mut self.archetypes[archetype_index];
        let location = EntityLocation {
            archetype: archetype_index,
            row: archetype.len(),
        };
        bundle.write(&ids, archetype);
        let entity = self.entities.alloc(location);
        self.archetypes[archetype_index].entities.push(entity);
        entity
    }

    /// Despawns `entity`. Returns `false` if it was not alive.
    ///
    /// The last entity of the archetype is swapped into the freed row.
    pub fn despawn(&mut self, entity: Entity) -> bool {
        let Some(location) = self.entities.free(entity) else {
            return false;
        };
        if let Some(moved) = self.archetypes[location.archetype].swap_remove(location.row) {
            self.entities.set_location(moved, location);
        }
        true
    }

    /// Whether `entity` is alive (spawned, not despawned, current generation).
    #[must_use]
    pub fn is_alive(&self, entity: Entity) -> bool {
        self.entities.location(entity).is_some()
    }

    /// Inserts `component` into `entity`, replacing an existing value of the same type in place.
    ///
    /// Adding a new component type moves the entity to another archetype (swap-remove in the
    /// source archetype, append in the target).
    ///
    /// # Errors
    ///
    /// [`EcsError::NoSuchEntity`] if `entity` is not alive; the world is left unchanged.
    pub fn insert<C: Component>(&mut self, entity: Entity, component: C) -> Result<(), EcsError> {
        let location = self
            .entities
            .location(entity)
            .ok_or(EcsError::NoSuchEntity(entity))?;
        let id = self.components.register::<C>();
        if let Some(column) = self.archetypes[location.archetype].column_mut::<C>(id) {
            column[location.row] = component;
            return Ok(());
        }
        let target = self.insert_target(location.archetype, id);
        let [source, destination] = self.disjoint_archetypes(location.archetype, target);
        let new_row = destination.len();
        let moved = source.move_row(location.row, destination, None);
        destination.push_component(id, component);
        self.relocate(entity, target, new_row, moved, location);
        Ok(())
    }

    /// Removes component `C` from `entity` and returns it.
    ///
    /// Returns `None` if the entity is not alive or has no `C`. Removing the last component
    /// keeps the entity alive in the component-less archetype.
    pub fn remove<C: Component>(&mut self, entity: Entity) -> Option<C> {
        let location = self.entities.location(entity)?;
        let id = self.components.id::<C>()?;
        self.archetypes[location.archetype].column::<C>(id)?;
        let target = self.remove_target(location.archetype, id);
        let [source, destination] = self.disjoint_archetypes(location.archetype, target);
        let value = source.column_mut::<C>(id)?.swap_remove(location.row);
        let new_row = destination.len();
        let moved = source.move_row(location.row, destination, Some(id));
        self.relocate(entity, target, new_row, moved, location);
        Some(value)
    }

    /// Component `C` of `entity`, if the entity is alive and has it.
    #[must_use]
    pub fn get<C: Component>(&self, entity: Entity) -> Option<&C> {
        let location = self.entities.location(entity)?;
        let id = self.components.id::<C>()?;
        self.archetypes[location.archetype]
            .column::<C>(id)?
            .get(location.row)
    }

    /// Mutable component `C` of `entity`, if the entity is alive and has it.
    pub fn get_mut<C: Component>(&mut self, entity: Entity) -> Option<&mut C> {
        let location = self.entities.location(entity)?;
        let id = self.components.id::<C>()?;
        self.archetypes[location.archetype]
            .column_mut::<C>(id)?
            .get_mut(location.row)
    }

    /// Number of live entities.
    #[must_use]
    pub fn entity_count(&self) -> usize {
        self.entities.alive()
    }

    /// Read-only query, e.g. `world.query::<(Entity, &Pos, &Vel)>()`.
    ///
    /// Visits archetypes in creation order and rows in dense order.
    pub fn query<Q: ReadOnlyQuery>(&self) -> QueryIter<'_, Q> {
        QueryIter::new(&self.components, &self.archetypes)
    }

    /// Query with mutable access, e.g. `world.query_mut::<(&mut Pos, &Vel)>()`.
    ///
    /// # Panics
    ///
    /// When the query accesses one component type mutably more than once, or mutably and
    /// shared at the same time (e.g. `(&mut Pos, &Pos)`).
    pub fn query_mut<Q: Query>(&mut self) -> QueryIterMut<'_, Q> {
        QueryIterMut::new(&self.components, &mut self.archetypes)
    }

    /// Inserts or replaces resource `R`. Replacing keeps the registration position.
    pub fn insert_resource<R: Resource>(&mut self, resource: R) {
        self.resources.insert(resource);
    }

    /// Resource `R`, if present.
    #[must_use]
    pub fn resource<R: Resource>(&self) -> Option<&R> {
        self.resources.get()
    }

    /// Mutable resource `R`, if present.
    pub fn resource_mut<R: Resource>(&mut self) -> Option<&mut R> {
        self.resources.get_mut()
    }

    /// Removes resource `R`. Its registration position is kept for a later re-insert.
    pub fn remove_resource<R: Resource>(&mut self) -> Option<R> {
        self.resources.remove()
    }

    /// Feeds the complete simulation state into `hasher`.
    ///
    /// Order: entity allocator (slot count; per slot generation and liveness; free list in reuse
    /// order), number of registered component types, archetypes in creation order (component
    /// ids, entities and every column's rows in dense order), resource slots in registration
    /// order with a presence tag. Types are identified by registration number only.
    pub fn stable_hash(&self, hasher: &mut StableHasher) {
        self.entities.stable_hash(hasher);
        hasher.write_usize(self.components.count());
        hasher.write_usize(self.archetypes.len());
        for archetype in &self.archetypes {
            archetype.stable_hash(hasher);
        }
        self.resources.stable_hash(hasher);
    }

    /// Captures the complete state: allocator, registries, archetypes, columns and resources.
    #[must_use]
    pub fn snapshot(&self) -> WorldSnapshot {
        WorldSnapshot {
            world: self.duplicate(),
        }
    }

    /// Replaces the complete state with `snapshot`.
    ///
    /// Afterwards [`World::stable_hash`] equals the hash at snapshot time and identical
    /// operations produce identical entity ids.
    pub fn restore(&mut self, snapshot: &WorldSnapshot) {
        *self = snapshot.world.duplicate();
    }

    fn duplicate(&self) -> Self {
        Self {
            entities: self.entities.clone(),
            components: self.components.clone(),
            archetypes: self.archetypes.clone(),
            archetype_index: self.archetype_index.clone(),
            resources: self.resources.clone(),
        }
    }

    fn archetype_for(&mut self, components: &[ComponentId]) -> usize {
        if let Some(&index) = self.archetype_index.get(components) {
            return index;
        }
        let columns = components
            .iter()
            .map(|&id| self.components.new_column(id))
            .collect();
        let index = self.archetypes.len();
        self.archetypes
            .push(Archetype::new(components.into(), columns));
        self.archetype_index.insert(components.into(), index);
        index
    }

    fn insert_target(&mut self, source: usize, id: ComponentId) -> usize {
        if let Some(&target) = self.archetypes[source].insert_edges.get(&id) {
            return target;
        }
        let mut components = self.archetypes[source].components.to_vec();
        if let Err(position) = components.binary_search(&id) {
            components.insert(position, id);
        }
        let target = self.archetype_for(&components);
        self.archetypes[source].insert_edges.insert(id, target);
        target
    }

    fn remove_target(&mut self, source: usize, id: ComponentId) -> usize {
        if let Some(&target) = self.archetypes[source].remove_edges.get(&id) {
            return target;
        }
        let mut components = self.archetypes[source].components.to_vec();
        components.retain(|&component| component != id);
        let target = self.archetype_for(&components);
        self.archetypes[source].remove_edges.insert(id, target);
        target
    }

    fn disjoint_archetypes(&mut self, source: usize, target: usize) -> [&mut Archetype; 2] {
        // Invariant: source and target differ in exactly the moved component, so the indices
        // are distinct and both in bounds.
        match self.archetypes.get_disjoint_mut([source, target]) {
            Ok(pair) => pair,
            Err(error) => panic!("internal ECS invariant violated: archetype move {error}"),
        }
    }

    fn relocate(
        &mut self,
        entity: Entity,
        target: usize,
        new_row: usize,
        moved: Option<Entity>,
        old: EntityLocation,
    ) {
        if let Some(moved) = moved {
            self.entities.set_location(moved, old);
        }
        self.entities.set_location(
            entity,
            EntityLocation {
                archetype: target,
                row: new_row,
            },
        );
    }
}

impl StableHash for World {
    fn stable_hash(&self, hasher: &mut StableHasher) {
        World::stable_hash(self, hasher);
    }
}

/// Complete, independent copy of a [`World`]'s state; see [`World::snapshot`].
pub struct WorldSnapshot {
    world: World,
}

impl Clone for WorldSnapshot {
    fn clone(&self) -> Self {
        Self {
            world: self.world.duplicate(),
        }
    }
}

impl fmt::Debug for WorldSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("WorldSnapshot").field(&self.world).finish()
    }
}
