//! Component trait, component registry and type-erased column storage.

use std::any::{Any, TypeId, type_name};
use std::collections::BTreeMap;

use grimoire_core::{StableHash, StableHasher};

/// Data attached to entities.
///
/// Blanket-implemented for every `'static + Send + Sync + Clone + StableHash` type, so plain
/// structs only need `#[derive(Clone)]` plus [`grimoire_core::impl_stable_hash!`]. `Clone` and
/// `StableHash` are required because worlds are snapshotted and hashed as a whole.
pub trait Component: 'static + Send + Sync + Clone + StableHash {}

impl<T: 'static + Send + Sync + Clone + StableHash> Component for T {}

/// Dense, registration-ordered identifier of a component type within one world.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ComponentId(u32);

impl ComponentId {
    /// Raw registration number.
    pub(crate) const fn raw(self) -> u32 {
        self.0
    }
}

/// Type-erased storage of one component column (`Vec<T>`).
pub trait ColumnStorage: Any + Send + Sync + 'static {
    /// Number of stored rows.
    fn row_count(&self) -> usize;
    /// Removes `row` by swapping in the last row and drops the value.
    fn swap_remove_drop(&mut self, row: usize);
    /// Removes `row` by swapping in the last row and pushes the value onto `destination`,
    /// which must store the same component type.
    fn swap_remove_into(&mut self, row: usize, destination: &mut dyn ColumnStorage);
    /// Deep copy of the column.
    fn clone_storage(&self) -> Box<dyn ColumnStorage>;
    /// Feeds every row in dense order.
    fn hash_rows(&self, hasher: &mut StableHasher);
}

/// Concrete column of components of type `T`.
pub struct Column<T> {
    pub(crate) data: Vec<T>,
}

impl<T: Component> ColumnStorage for Column<T> {
    fn row_count(&self) -> usize {
        self.data.len()
    }

    fn swap_remove_drop(&mut self, row: usize) {
        self.data.swap_remove(row);
    }

    fn swap_remove_into(&mut self, row: usize, destination: &mut dyn ColumnStorage) {
        let value = self.data.swap_remove(row);
        let destination: &mut dyn Any = destination;
        // Invariant: both columns belong to the same ComponentId, which maps to exactly one type
        // per world. A mismatch would silently misalign rows, so it is a hard error.
        match destination.downcast_mut::<Self>() {
            Some(column) => column.data.push(value),
            None => panic!(
                "internal ECS invariant violated: column type mismatch while moving `{}`",
                type_name::<T>()
            ),
        }
    }

    fn clone_storage(&self) -> Box<dyn ColumnStorage> {
        Box::new(Self {
            data: self.data.clone(),
        })
    }

    fn hash_rows(&self, hasher: &mut StableHasher) {
        for value in &self.data {
            value.stable_hash(hasher);
        }
    }
}

fn new_column<C: Component>() -> Box<dyn ColumnStorage> {
    Box::new(Column::<C> { data: Vec::new() })
}

#[derive(Clone)]
struct ComponentInfo {
    name: &'static str,
    new_column: fn() -> Box<dyn ColumnStorage>,
}

/// Maps component types to registration-ordered [`ComponentId`]s.
///
/// `TypeId` is only used as a lookup key; ids, hashes and iteration never depend on it.
#[derive(Clone, Default)]
pub struct ComponentRegistry {
    ids: BTreeMap<TypeId, ComponentId>,
    infos: Vec<ComponentInfo>,
}

impl ComponentRegistry {
    /// Returns the id of `C`, registering it with the next free id if necessary.
    pub fn register<C: Component>(&mut self) -> ComponentId {
        if let Some(&id) = self.ids.get(&TypeId::of::<C>()) {
            return id;
        }
        let Ok(raw) = u32::try_from(self.infos.len()) else {
            panic!("more than u32::MAX component types registered");
        };
        let id = ComponentId(raw);
        self.infos.push(ComponentInfo {
            name: type_name::<C>(),
            new_column: new_column::<C>,
        });
        self.ids.insert(TypeId::of::<C>(), id);
        id
    }

    /// Id of `C` if it has been registered.
    pub fn id<C: Component>(&self) -> Option<ComponentId> {
        self.ids.get(&TypeId::of::<C>()).copied()
    }

    /// Number of registered component types.
    pub fn count(&self) -> usize {
        self.infos.len()
    }

    /// Creates an empty column for `id`.
    pub fn new_column(&self, id: ComponentId) -> Box<dyn ColumnStorage> {
        (self.infos[id.raw() as usize].new_column)()
    }

    /// Type name of `id`, for diagnostics only.
    pub fn name(&self, id: ComponentId) -> &'static str {
        self.infos[id.raw() as usize].name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_is_ordered_and_idempotent() {
        let mut registry = ComponentRegistry::default();
        let a = registry.register::<u32>();
        let b = registry.register::<String>();
        assert_eq!(registry.register::<u32>(), a);
        assert_eq!(a.raw(), 0);
        assert_eq!(b.raw(), 1);
        assert_eq!(registry.count(), 2);
        assert_eq!(registry.id::<String>(), Some(b));
        assert_eq!(registry.id::<u8>(), None);
        assert!(registry.name(b).contains("String"));
    }

    #[test]
    fn columns_move_and_clone() {
        let mut registry = ComponentRegistry::default();
        let id = registry.register::<u32>();
        let mut source = registry.new_column(id);
        let mut destination = registry.new_column(id);
        let any: &mut dyn Any = &mut *source;
        let Some(column) = any.downcast_mut::<Column<u32>>() else {
            panic!("downcast failed");
        };
        column.data.extend([1, 2, 3]);
        source.swap_remove_into(0, &mut *destination);
        assert_eq!(source.row_count(), 2);
        assert_eq!(destination.row_count(), 1);
        let copy = source.clone_storage();
        source.swap_remove_drop(0);
        assert_eq!(source.row_count(), 1);
        assert_eq!(copy.row_count(), 2);
        let mut left = StableHasher::new();
        copy.hash_rows(&mut left);
        let mut right = StableHasher::new();
        3u32.stable_hash(&mut right);
        2u32.stable_hash(&mut right);
        assert_eq!(left.finish(), right.finish());
    }

    #[test]
    #[should_panic(expected = "column type mismatch")]
    fn moving_between_different_types_panics() {
        let mut registry = ComponentRegistry::default();
        let a = registry.register::<u32>();
        let b = registry.register::<u64>();
        let mut source = registry.new_column(a);
        let mut destination = registry.new_column(b);
        let any: &mut dyn Any = &mut *source;
        if let Some(column) = any.downcast_mut::<Column<u32>>() {
            column.data.push(1);
        }
        source.swap_remove_into(0, &mut *destination);
    }
}
