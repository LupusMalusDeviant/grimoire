//! Archetype tables: one row per entity, one column per component type.

use std::any::Any;
use std::collections::BTreeMap;

use grimoire_core::StableHasher;

use crate::component::{Column, ColumnStorage, Component, ComponentId};
use crate::entity::Entity;

/// Table of all entities sharing exactly the same component set.
///
/// `components` is sorted ascending; `columns[i]` stores the component `components[i]`.
/// All columns and `entities` always have the same length.
pub struct Archetype {
    pub(crate) components: Box<[ComponentId]>,
    pub(crate) entities: Vec<Entity>,
    pub(crate) columns: Vec<Box<dyn ColumnStorage>>,
    pub(crate) insert_edges: BTreeMap<ComponentId, usize>,
    pub(crate) remove_edges: BTreeMap<ComponentId, usize>,
}

impl Clone for Archetype {
    fn clone(&self) -> Self {
        Self {
            components: self.components.clone(),
            entities: self.entities.clone(),
            columns: self
                .columns
                .iter()
                .map(|column| column.clone_storage())
                .collect(),
            insert_edges: self.insert_edges.clone(),
            remove_edges: self.remove_edges.clone(),
        }
    }
}

impl Archetype {
    pub(crate) fn new(
        components: Box<[ComponentId]>,
        columns: Vec<Box<dyn ColumnStorage>>,
    ) -> Self {
        debug_assert!(components.windows(2).all(|pair| pair[0] < pair[1]));
        debug_assert_eq!(components.len(), columns.len());
        Self {
            components,
            entities: Vec::new(),
            columns,
            insert_edges: BTreeMap::new(),
            remove_edges: BTreeMap::new(),
        }
    }

    /// Number of entities stored.
    pub(crate) fn len(&self) -> usize {
        self.entities.len()
    }

    /// Column index of `id`, if the archetype contains it.
    pub(crate) fn column_index(&self, id: ComponentId) -> Option<usize> {
        self.components.binary_search(&id).ok()
    }

    pub(crate) fn column<C: Component>(&self, id: ComponentId) -> Option<&[C]> {
        let storage: &dyn ColumnStorage = &*self.columns[self.column_index(id)?];
        let any: &dyn Any = storage;
        any.downcast_ref::<Column<C>>()
            .map(|column| column.data.as_slice())
    }

    pub(crate) fn column_mut<C: Component>(&mut self, id: ComponentId) -> Option<&mut Vec<C>> {
        let index = self.column_index(id)?;
        let storage: &mut dyn ColumnStorage = &mut *self.columns[index];
        let any: &mut dyn Any = storage;
        any.downcast_mut::<Column<C>>()
            .map(|column| &mut column.data)
    }

    /// Pushes one component while a new row is being assembled.
    pub(crate) fn push_component<C: Component>(&mut self, id: ComponentId, value: C) {
        // Invariant: the archetype was selected for exactly this component set.
        match self.column_mut::<C>(id) {
            Some(column) => column.push(value),
            None => panic!(
                "internal ECS invariant violated: archetype has no column for `{}`",
                std::any::type_name::<C>()
            ),
        }
    }

    /// Swap-removes `row` from every column and returns the entity moved into `row`, if any.
    pub(crate) fn swap_remove(&mut self, row: usize) -> Option<Entity> {
        for column in &mut self.columns {
            column.swap_remove_drop(row);
        }
        self.entities.swap_remove(row);
        self.entities.get(row).copied()
    }

    /// Moves `row` into `destination`, dropping columns the destination lacks.
    ///
    /// The column of `skip` must already have been swap-removed by the caller. Returns the
    /// entity moved into `row` of `self`, if any.
    pub(crate) fn move_row(
        &mut self,
        row: usize,
        destination: &mut Self,
        skip: Option<ComponentId>,
    ) -> Option<Entity> {
        for (index, id) in self.components.iter().enumerate() {
            if Some(*id) == skip {
                continue;
            }
            match destination.column_index(*id) {
                Some(target) => {
                    self.columns[index].swap_remove_into(row, &mut *destination.columns[target]);
                }
                None => self.columns[index].swap_remove_drop(row),
            }
        }
        let entity = self.entities.swap_remove(row);
        destination.entities.push(entity);
        self.entities.get(row).copied()
    }

    /// Feeds component ids, entities and all column data in dense order.
    pub(crate) fn stable_hash(&self, hasher: &mut StableHasher) {
        hasher.write_usize(self.components.len());
        for id in &self.components {
            hasher.write_u32(id.raw());
        }
        hasher.write_usize(self.entities.len());
        for entity in &self.entities {
            hasher.write_u64(entity.to_bits());
        }
        for column in &self.columns {
            column.hash_rows(hasher);
        }
    }
}
