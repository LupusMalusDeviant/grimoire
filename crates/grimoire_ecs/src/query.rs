//! Typed queries over archetype storage.
//!
//! A query is a single element or a tuple of up to eight elements:
//!
//! | Element | Item | Matches |
//! |---------|------|---------|
//! | [`Entity`] | `Entity` | every entity |
//! | `&T` | `&T` | entities with `T` |
//! | `&mut T` | `&mut T` | entities with `T` (only [`World::query_mut`](crate::World::query_mut)) |
//! | `Option<&T>` | `Option<&T>` | every entity |
//! | `Option<&mut T>` | `Option<&mut T>` | every entity (only `query_mut`) |
//! | [`With<T>`] | `()` | entities with `T` |
//! | [`Without<T>`] | `()` | entities without `T` |
//!
//! Iteration visits archetypes in creation order and rows in dense order. Per archetype the
//! columns are downcast once; per entity only slice iterators advance, nothing is allocated.

use std::marker::PhantomData;
use std::slice;

use crate::archetype::Archetype;
use crate::component::{Component, ComponentId, ComponentRegistry};
use crate::entity::Entity;

use internal::{
    Element, QueryInternal, ReadOnlyElement, check_conflicts, exclusive_slots, shared_slots,
};

pub(crate) mod internal {
    use std::any::{Any, TypeId, type_name};

    use crate::archetype::Archetype;
    use crate::component::{Column, ColumnStorage, Component, ComponentId, ComponentRegistry};
    use crate::entity::Entity;

    /// Data access of one query element, used to reject aliasing queries.
    pub struct ComponentAccess {
        pub type_id: TypeId,
        pub name: &'static str,
        pub mutable: bool,
    }

    impl ComponentAccess {
        pub fn of<T: Component>(mutable: bool) -> Self {
            Self {
                type_id: TypeId::of::<T>(),
                name: type_name::<T>(),
                mutable,
            }
        }
    }

    /// A column handed to one query element for one archetype.
    #[derive(Default)]
    pub enum Slot<'w> {
        #[default]
        Empty,
        Shared(&'w dyn ColumnStorage),
        Exclusive(&'w mut dyn ColumnStorage),
    }

    impl<'w> Slot<'w> {
        pub fn shared<T: Component>(self) -> Option<&'w [T]> {
            let storage: &'w dyn ColumnStorage = match self {
                Slot::Empty => return None,
                Slot::Shared(storage) => storage,
                Slot::Exclusive(storage) => storage,
            };
            let any: &'w dyn Any = storage;
            any.downcast_ref::<Column<T>>()
                .map(|column| column.data.as_slice())
        }

        pub fn exclusive<T: Component>(self) -> Option<&'w mut [T]> {
            match self {
                Slot::Exclusive(storage) => {
                    let any: &'w mut dyn Any = storage;
                    any.downcast_mut::<Column<T>>()
                        .map(|column| column.data.as_mut_slice())
                }
                Slot::Empty | Slot::Shared(_) => None,
            }
        }

        pub const fn is_empty(&self) -> bool {
            matches!(self, Slot::Empty)
        }
    }

    /// One element of a query tuple; sealed.
    pub trait Element {
        type State;
        type Fetch<'w>;
        type Item<'w>;
        fn access() -> Option<ComponentAccess>;
        fn init_state(registry: &ComponentRegistry) -> Self::State;
        /// Component whose column the element reads or writes.
        fn column(state: &Self::State) -> Option<ComponentId>;
        fn matches(state: &Self::State, components: &[ComponentId]) -> bool;
        fn fetch<'w>(
            state: &Self::State,
            slot: Slot<'w>,
            entities: &'w [Entity],
        ) -> Option<Self::Fetch<'w>>;
        fn next<'w>(fetch: &mut Self::Fetch<'w>) -> Option<Self::Item<'w>>;
    }

    /// Elements that never write.
    pub trait ReadOnlyElement: Element {}

    /// Per-query machinery behind [`Query`](super::Query); sealed.
    pub trait QueryInternal {
        type State;
        type Fetch<'w>;
        fn check_access();
        fn init_state(registry: &ComponentRegistry) -> Self::State;
        fn matches(state: &Self::State, components: &[ComponentId]) -> bool;
        fn fetch_shared<'w>(
            state: &Self::State,
            archetype: &'w Archetype,
        ) -> Option<Self::Fetch<'w>>;
        fn fetch_exclusive<'w>(
            state: &Self::State,
            archetype: &'w mut Archetype,
        ) -> Option<Self::Fetch<'w>>;
    }

    pub fn check_conflicts<Q: ?Sized>(accesses: &[Option<ComponentAccess>]) {
        for (position, first) in accesses.iter().enumerate() {
            let Some(first) = first else { continue };
            for second in accesses[position + 1..].iter().flatten() {
                if first.type_id != second.type_id || !(first.mutable || second.mutable) {
                    continue;
                }
                if first.mutable && second.mutable {
                    panic!(
                        "query `{}` requests duplicate mutable access to component `{}`",
                        type_name::<Q>(),
                        first.name
                    );
                }
                panic!(
                    "query `{}` requests mutable and shared access to component `{}` at the same time",
                    type_name::<Q>(),
                    first.name
                );
            }
        }
    }

    pub fn shared_slots<'w, const N: usize>(
        archetype: &'w Archetype,
        wanted: [Option<ComponentId>; N],
    ) -> [Slot<'w>; N] {
        wanted.map(|id| match id.and_then(|id| archetype.column_index(id)) {
            Some(index) => Slot::Shared(&*archetype.columns[index]),
            None => Slot::Empty,
        })
    }

    /// Splits the columns of `archetype` into disjoint slots, one per query element.
    ///
    /// A column wanted by exactly one element is handed out exclusively; a column wanted by
    /// several elements (only possible for shared access after `check_access`) is shared.
    pub fn exclusive_slots<'w, const N: usize>(
        archetype: &'w mut Archetype,
        wanted: [Option<ComponentId>; N],
    ) -> ([Slot<'w>; N], &'w [Entity]) {
        let Archetype {
            components,
            entities,
            columns,
            ..
        } = archetype;
        let indices = wanted.map(|id| id.and_then(|id| components.binary_search(&id).ok()));
        let mut slots: [Slot<'w>; N] = std::array::from_fn(|_| Slot::Empty);
        for (column_index, column) in columns.iter_mut().enumerate() {
            let wanted_here = Some(column_index);
            match indices
                .iter()
                .filter(|&&index| index == wanted_here)
                .count()
            {
                0 => {}
                1 => {
                    if let Some(element) = indices.iter().position(|&index| index == wanted_here) {
                        slots[element] = Slot::Exclusive(&mut **column);
                    }
                }
                _ => {
                    let shared: &'w dyn ColumnStorage = &**column;
                    for (element, index) in indices.iter().enumerate() {
                        if *index == wanted_here {
                            slots[element] = Slot::Shared(shared);
                        }
                    }
                }
            }
        }
        (slots, entities.as_slice())
    }
}

/// Filter element: matches entities that have component `T`, yields `()`.
pub struct With<T>(PhantomData<fn() -> T>);

/// Filter element: matches entities that do not have component `T`, yields `()`.
pub struct Without<T>(PhantomData<fn() -> T>);

/// A query: a single element or a tuple of up to eight elements.
///
/// | Element | Item | Matches |
/// |---------|------|---------|
/// | [`Entity`] | `Entity` | every entity |
/// | `&T` | `&T` | entities with `T` |
/// | `&mut T` | `&mut T` | entities with `T` (only [`World::query_mut`](crate::World::query_mut)) |
/// | `Option<&T>` | `Option<&T>` | every entity |
/// | `Option<&mut T>` | `Option<&mut T>` | every entity (only `query_mut`) |
/// | [`With<T>`] | `()` | entities with `T` |
/// | [`Without<T>`] | `()` | entities without `T` |
///
/// Iteration visits archetypes in creation order and rows in dense order. Columns are downcast
/// once per archetype; per entity only slice iterators advance and nothing is allocated.
///
/// The trait is sealed and cannot be implemented outside this crate.
pub trait Query: QueryInternal {
    /// Value yielded for each matching entity.
    type Item<'w>;

    #[doc(hidden)]
    fn next_item<'w>(fetch: &mut Self::Fetch<'w>) -> Option<Self::Item<'w>>;
}

/// A [`Query`] without mutable elements, usable with [`World::query`](crate::World::query).
pub trait ReadOnlyQuery: Query {}

fn contains(components: &[ComponentId], id: ComponentId) -> bool {
    components.binary_search(&id).is_ok()
}

impl Element for Entity {
    type State = ();
    type Fetch<'w> = slice::Iter<'w, Entity>;
    type Item<'w> = Entity;

    fn access() -> Option<internal::ComponentAccess> {
        None
    }

    fn init_state(_registry: &ComponentRegistry) -> Self::State {}

    fn column(_state: &Self::State) -> Option<ComponentId> {
        None
    }

    fn matches(_state: &Self::State, _components: &[ComponentId]) -> bool {
        true
    }

    fn fetch<'w>(
        _state: &Self::State,
        _slot: internal::Slot<'w>,
        entities: &'w [Entity],
    ) -> Option<Self::Fetch<'w>> {
        Some(entities.iter())
    }

    #[inline]
    fn next<'w>(fetch: &mut Self::Fetch<'w>) -> Option<Self::Item<'w>> {
        fetch.next().copied()
    }
}

impl ReadOnlyElement for Entity {}

impl<T: Component> Element for &T {
    type State = Option<ComponentId>;
    type Fetch<'w> = slice::Iter<'w, T>;
    type Item<'w> = &'w T;

    fn access() -> Option<internal::ComponentAccess> {
        Some(internal::ComponentAccess::of::<T>(false))
    }

    fn init_state(registry: &ComponentRegistry) -> Self::State {
        registry.id::<T>()
    }

    fn column(state: &Self::State) -> Option<ComponentId> {
        *state
    }

    fn matches(state: &Self::State, components: &[ComponentId]) -> bool {
        state.is_some_and(|id| contains(components, id))
    }

    fn fetch<'w>(
        _state: &Self::State,
        slot: internal::Slot<'w>,
        _entities: &'w [Entity],
    ) -> Option<Self::Fetch<'w>> {
        slot.shared::<T>().map(<[T]>::iter)
    }

    #[inline]
    fn next<'w>(fetch: &mut Self::Fetch<'w>) -> Option<Self::Item<'w>> {
        fetch.next()
    }
}

impl<T: Component> ReadOnlyElement for &T {}

impl<T: Component> Element for &mut T {
    type State = Option<ComponentId>;
    type Fetch<'w> = slice::IterMut<'w, T>;
    type Item<'w> = &'w mut T;

    fn access() -> Option<internal::ComponentAccess> {
        Some(internal::ComponentAccess::of::<T>(true))
    }

    fn init_state(registry: &ComponentRegistry) -> Self::State {
        registry.id::<T>()
    }

    fn column(state: &Self::State) -> Option<ComponentId> {
        *state
    }

    fn matches(state: &Self::State, components: &[ComponentId]) -> bool {
        state.is_some_and(|id| contains(components, id))
    }

    fn fetch<'w>(
        _state: &Self::State,
        slot: internal::Slot<'w>,
        _entities: &'w [Entity],
    ) -> Option<Self::Fetch<'w>> {
        slot.exclusive::<T>().map(<[T]>::iter_mut)
    }

    #[inline]
    fn next<'w>(fetch: &mut Self::Fetch<'w>) -> Option<Self::Item<'w>> {
        fetch.next()
    }
}

impl<T: Component> Element for Option<&T> {
    type State = Option<ComponentId>;
    type Fetch<'w> = Option<slice::Iter<'w, T>>;
    type Item<'w> = Option<&'w T>;

    fn access() -> Option<internal::ComponentAccess> {
        Some(internal::ComponentAccess::of::<T>(false))
    }

    fn init_state(registry: &ComponentRegistry) -> Self::State {
        registry.id::<T>()
    }

    fn column(state: &Self::State) -> Option<ComponentId> {
        *state
    }

    fn matches(_state: &Self::State, _components: &[ComponentId]) -> bool {
        true
    }

    fn fetch<'w>(
        _state: &Self::State,
        slot: internal::Slot<'w>,
        _entities: &'w [Entity],
    ) -> Option<Self::Fetch<'w>> {
        Some(slot.shared::<T>().map(<[T]>::iter))
    }

    #[inline]
    fn next<'w>(fetch: &mut Self::Fetch<'w>) -> Option<Self::Item<'w>> {
        match fetch {
            Some(iter) => iter.next().map(Some),
            None => Some(None),
        }
    }
}

impl<T: Component> ReadOnlyElement for Option<&T> {}

impl<T: Component> Element for Option<&mut T> {
    type State = Option<ComponentId>;
    type Fetch<'w> = Option<slice::IterMut<'w, T>>;
    type Item<'w> = Option<&'w mut T>;

    fn access() -> Option<internal::ComponentAccess> {
        Some(internal::ComponentAccess::of::<T>(true))
    }

    fn init_state(registry: &ComponentRegistry) -> Self::State {
        registry.id::<T>()
    }

    fn column(state: &Self::State) -> Option<ComponentId> {
        *state
    }

    fn matches(_state: &Self::State, _components: &[ComponentId]) -> bool {
        true
    }

    fn fetch<'w>(
        _state: &Self::State,
        slot: internal::Slot<'w>,
        _entities: &'w [Entity],
    ) -> Option<Self::Fetch<'w>> {
        if slot.is_empty() {
            return Some(None);
        }
        slot.exclusive::<T>().map(|column| Some(column.iter_mut()))
    }

    #[inline]
    fn next<'w>(fetch: &mut Self::Fetch<'w>) -> Option<Self::Item<'w>> {
        match fetch {
            Some(iter) => iter.next().map(Some),
            None => Some(None),
        }
    }
}

impl<T: Component> Element for With<T> {
    type State = Option<ComponentId>;
    type Fetch<'w> = ();
    type Item<'w> = ();

    fn access() -> Option<internal::ComponentAccess> {
        None
    }

    fn init_state(registry: &ComponentRegistry) -> Self::State {
        registry.id::<T>()
    }

    fn column(_state: &Self::State) -> Option<ComponentId> {
        None
    }

    fn matches(state: &Self::State, components: &[ComponentId]) -> bool {
        state.is_some_and(|id| contains(components, id))
    }

    fn fetch<'w>(
        _state: &Self::State,
        _slot: internal::Slot<'w>,
        _entities: &'w [Entity],
    ) -> Option<Self::Fetch<'w>> {
        Some(())
    }

    #[inline]
    fn next<'w>(_fetch: &mut Self::Fetch<'w>) -> Option<Self::Item<'w>> {
        Some(())
    }
}

impl<T: Component> ReadOnlyElement for With<T> {}

impl<T: Component> Element for Without<T> {
    type State = Option<ComponentId>;
    type Fetch<'w> = ();
    type Item<'w> = ();

    fn access() -> Option<internal::ComponentAccess> {
        None
    }

    fn init_state(registry: &ComponentRegistry) -> Self::State {
        registry.id::<T>()
    }

    fn column(_state: &Self::State) -> Option<ComponentId> {
        None
    }

    fn matches(state: &Self::State, components: &[ComponentId]) -> bool {
        state.is_none_or(|id| !contains(components, id))
    }

    fn fetch<'w>(
        _state: &Self::State,
        _slot: internal::Slot<'w>,
        _entities: &'w [Entity],
    ) -> Option<Self::Fetch<'w>> {
        Some(())
    }

    #[inline]
    fn next<'w>(_fetch: &mut Self::Fetch<'w>) -> Option<Self::Item<'w>> {
        Some(())
    }
}

impl<T: Component> ReadOnlyElement for Without<T> {}

impl<E: Element> QueryInternal for E {
    type State = <E as Element>::State;
    type Fetch<'w> = <E as Element>::Fetch<'w>;

    fn check_access() {}

    fn init_state(registry: &ComponentRegistry) -> Self::State {
        <E as Element>::init_state(registry)
    }

    fn matches(state: &Self::State, components: &[ComponentId]) -> bool {
        <E as Element>::matches(state, components)
    }

    fn fetch_shared<'w>(state: &Self::State, archetype: &'w Archetype) -> Option<Self::Fetch<'w>> {
        let [slot] = shared_slots(archetype, [<E as Element>::column(state)]);
        <E as Element>::fetch(state, slot, &archetype.entities)
    }

    fn fetch_exclusive<'w>(
        state: &Self::State,
        archetype: &'w mut Archetype,
    ) -> Option<Self::Fetch<'w>> {
        let ([slot], entities) = exclusive_slots(archetype, [<E as Element>::column(state)]);
        <E as Element>::fetch(state, slot, entities)
    }
}

impl<E: Element> Query for E {
    type Item<'w> = <E as Element>::Item<'w>;

    #[inline]
    fn next_item<'w>(fetch: &mut Self::Fetch<'w>) -> Option<Self::Item<'w>> {
        <E as Element>::next(fetch)
    }
}

impl<E: ReadOnlyElement> ReadOnlyQuery for E {}

macro_rules! impl_query_tuple {
    ($(($E:ident, $index:tt)),+) => {
        impl<$($E: Element),+> QueryInternal for ($($E,)+) {
            type State = ($(<$E as Element>::State,)+);
            type Fetch<'w> = ($(<$E as Element>::Fetch<'w>,)+);

            fn check_access() {
                check_conflicts::<Self>(&[$(<$E as Element>::access()),+]);
            }

            fn init_state(registry: &ComponentRegistry) -> Self::State {
                ($(<$E as Element>::init_state(registry),)+)
            }

            fn matches(state: &Self::State, components: &[ComponentId]) -> bool {
                $(<$E as Element>::matches(&state.$index, components))&&+
            }

            fn fetch_shared<'w>(
                state: &Self::State,
                archetype: &'w Archetype,
            ) -> Option<Self::Fetch<'w>> {
                let mut slots =
                    shared_slots(archetype, [$(<$E as Element>::column(&state.$index)),+]);
                Some(($(
                    <$E as Element>::fetch(
                        &state.$index,
                        std::mem::take(&mut slots[$index]),
                        &archetype.entities,
                    )?,
                )+))
            }

            fn fetch_exclusive<'w>(
                state: &Self::State,
                archetype: &'w mut Archetype,
            ) -> Option<Self::Fetch<'w>> {
                let (mut slots, entities) =
                    exclusive_slots(archetype, [$(<$E as Element>::column(&state.$index)),+]);
                Some(($(
                    <$E as Element>::fetch(
                        &state.$index,
                        std::mem::take(&mut slots[$index]),
                        entities,
                    )?,
                )+))
            }
        }

        impl<$($E: Element),+> Query for ($($E,)+) {
            type Item<'w> = ($(<$E as Element>::Item<'w>,)+);

            #[inline]
            fn next_item<'w>(fetch: &mut Self::Fetch<'w>) -> Option<Self::Item<'w>> {
                Some(($(<$E as Element>::next(&mut fetch.$index)?,)+))
            }
        }

        impl<$($E: ReadOnlyElement),+> ReadOnlyQuery for ($($E,)+) {}
    };
}

impl_query_tuple!((A, 0));
impl_query_tuple!((A, 0), (B, 1));
impl_query_tuple!((A, 0), (B, 1), (C, 2));
impl_query_tuple!((A, 0), (B, 1), (C, 2), (D, 3));
impl_query_tuple!((A, 0), (B, 1), (C, 2), (D, 3), (E, 4));
impl_query_tuple!((A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5));
impl_query_tuple!((A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5), (G, 6));
impl_query_tuple!(
    (A, 0),
    (B, 1),
    (C, 2),
    (D, 3),
    (E, 4),
    (F, 5),
    (G, 6),
    (H, 7)
);

/// Iterator returned by [`World::query`](crate::World::query).
pub struct QueryIter<'w, Q: Query> {
    state: Q::State,
    archetypes: slice::Iter<'w, Archetype>,
    current: Option<Q::Fetch<'w>>,
    remaining: usize,
}

impl<'w, Q: Query> QueryIter<'w, Q> {
    pub(crate) fn new(registry: &ComponentRegistry, archetypes: &'w [Archetype]) -> Self {
        Q::check_access();
        Self {
            state: Q::init_state(registry),
            archetypes: archetypes.iter(),
            current: None,
            remaining: 0,
        }
    }
}

impl<'w, Q: Query> Iterator for QueryIter<'w, Q> {
    type Item = Q::Item<'w>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.remaining > 0 {
                self.remaining -= 1;
                if let Some(item) = self.current.as_mut().and_then(Q::next_item) {
                    return Some(item);
                }
                self.remaining = 0;
            }
            let archetype = self.archetypes.next()?;
            if archetype.entities.is_empty() || !Q::matches(&self.state, &archetype.components) {
                continue;
            }
            self.current = Q::fetch_shared(&self.state, archetype);
            self.remaining = if self.current.is_some() {
                archetype.len()
            } else {
                0
            };
        }
    }
}

/// Iterator returned by [`World::query_mut`](crate::World::query_mut).
pub struct QueryIterMut<'w, Q: Query> {
    state: Q::State,
    archetypes: slice::IterMut<'w, Archetype>,
    current: Option<Q::Fetch<'w>>,
    remaining: usize,
}

impl<'w, Q: Query> QueryIterMut<'w, Q> {
    pub(crate) fn new(registry: &ComponentRegistry, archetypes: &'w mut [Archetype]) -> Self {
        Q::check_access();
        Self {
            state: Q::init_state(registry),
            archetypes: archetypes.iter_mut(),
            current: None,
            remaining: 0,
        }
    }
}

impl<'w, Q: Query> Iterator for QueryIterMut<'w, Q> {
    type Item = Q::Item<'w>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.remaining > 0 {
                self.remaining -= 1;
                if let Some(item) = self.current.as_mut().and_then(Q::next_item) {
                    return Some(item);
                }
                self.remaining = 0;
            }
            let archetype = self.archetypes.next()?;
            if archetype.entities.is_empty() || !Q::matches(&self.state, &archetype.components) {
                continue;
            }
            let len = archetype.len();
            self.current = Q::fetch_exclusive(&self.state, archetype);
            self.remaining = if self.current.is_some() { len } else { 0 };
        }
    }
}
