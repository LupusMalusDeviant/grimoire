//! Declared data access of parallel systems (engine ADR-0006, building block 1).

use std::any::{TypeId, type_name};

use crate::component::Component;
use crate::resource::Resource;

/// One declared type. `TypeId` is only a lookup key; stages compare schedule-local registration
/// numbers, never `TypeId` order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Declared {
    pub(crate) type_id: TypeId,
    pub(crate) name: &'static str,
    pub(crate) write: bool,
}

/// Declared reads, deferred writes and structural commands of a
/// [`ParallelSystem`](crate::ParallelSystem).
///
/// | Declaration | Covers |
/// |-------------|--------|
/// | [`Access::read`] | `&C` and `Option<&C>` in [`World::query`](crate::World::query) and [`World::par_blocks`](crate::World::par_blocks), [`World::get`](crate::World::get) |
/// | [`Access::write`] | [`CommandBuffer::set`](crate::CommandBuffer::set) |
/// | [`Access::read_resource`] | [`World::resource`](crate::World::resource) |
/// | [`Access::write_resource`] | [`CommandBuffer::insert_resource`](crate::CommandBuffer::insert_resource), [`CommandBuffer::remove_resource`](crate::CommandBuffer::remove_resource) |
/// | [`Access::structural`] | [`CommandBuffer::spawn`](crate::CommandBuffer::spawn), `despawn`, `insert`, `remove` |
///
/// `write` does not imply `read`. The query element [`Entity`](crate::Entity), the filters
/// [`With`](crate::With) and [`Without`](crate::Without), [`World::is_alive`](crate::World::is_alive),
/// [`World::entity_count`](crate::World::entity_count) and
/// [`World::executor`](crate::World::executor) need no declaration: membership and existence
/// change only through structural commands, which close a stage for every later system.
/// Declaring a type twice has no further effect.
///
/// ```
/// use grimoire_core::impl_stable_hash;
/// use grimoire_ecs::Access;
///
/// #[derive(Clone)]
/// struct Pos { x: f32 }
/// impl_stable_hash!(Pos { x });
///
/// let access = Access::new().read::<Pos>().write::<Pos>().structural();
/// assert_ne!(access, Access::new());
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Access {
    pub(crate) components: Vec<Declared>,
    pub(crate) resources: Vec<Declared>,
    pub(crate) structural: bool,
}

impl Access {
    /// An empty declaration: no reads, no writes, no structural commands.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Declares reading component `C`.
    #[must_use]
    pub fn read<C: Component>(mut self) -> Self {
        push(&mut self.components, declared::<C>(false));
        self
    }

    /// Declares writing component `C` through [`CommandBuffer::set`](crate::CommandBuffer::set).
    #[must_use]
    pub fn write<C: Component>(mut self) -> Self {
        push(&mut self.components, declared::<C>(true));
        self
    }

    /// Declares reading resource `R`.
    #[must_use]
    pub fn read_resource<R: Resource>(mut self) -> Self {
        push(&mut self.resources, declared::<R>(false));
        self
    }

    /// Declares writing resource `R` through
    /// [`CommandBuffer::insert_resource`](crate::CommandBuffer::insert_resource) or
    /// [`CommandBuffer::remove_resource`](crate::CommandBuffer::remove_resource).
    #[must_use]
    pub fn write_resource<R: Resource>(mut self) -> Self {
        push(&mut self.resources, declared::<R>(true));
        self
    }

    /// Declares recording structural commands (spawn, despawn, insert, remove).
    #[must_use]
    pub fn structural(mut self) -> Self {
        self.structural = true;
        self
    }
}

fn declared<T: 'static>(write: bool) -> Declared {
    Declared {
        type_id: TypeId::of::<T>(),
        name: type_name::<T>(),
        write,
    }
}

fn push(list: &mut Vec<Declared>, entry: Declared) {
    if !list.contains(&entry) {
        list.push(entry);
    }
}
