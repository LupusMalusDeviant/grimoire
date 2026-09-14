//! Bundles: tuples of components spawned together.

use crate::archetype::Archetype;
use crate::component::{Component, ComponentId, ComponentRegistry};

pub(crate) mod internal {
    use super::{Archetype, ComponentId, ComponentRegistry};

    /// Implementation details of [`Bundle`](super::Bundle); sealed.
    pub trait BundleInternal {
        /// Fixed-size array of component ids in tuple order.
        type Ids: AsRef<[ComponentId]> + AsMut<[ComponentId]> + Copy;
        /// Whether the tuple names one component type more than once. Must not touch any
        /// registry, so a rejected bundle leaves the world unchanged.
        fn has_duplicate_types() -> bool;
        /// Registers every component type in tuple order and returns their ids.
        fn register(registry: &mut ComponentRegistry) -> Self::Ids;
        /// Pushes every component into the matching column of `archetype`.
        fn write(self, ids: &Self::Ids, archetype: &mut Archetype);
    }
}

/// A set of components spawned together: `()` and tuples `(C1,)` up to `(C1, …, C8)`.
///
/// Component types that are not registered yet are registered in tuple order. A bundle must not
/// contain the same component type twice; spawning such a bundle panics before the world changes.
pub trait Bundle: internal::BundleInternal + Send + 'static {}

impl internal::BundleInternal for () {
    type Ids = [ComponentId; 0];

    fn has_duplicate_types() -> bool {
        false
    }

    fn register(_registry: &mut ComponentRegistry) -> Self::Ids {
        []
    }

    fn write(self, _ids: &Self::Ids, _archetype: &mut Archetype) {}
}

impl Bundle for () {}

macro_rules! impl_bundle {
    ($count:literal; $(($C:ident, $index:tt)),+) => {
        impl<$($C: Component),+> internal::BundleInternal for ($($C,)+) {
            type Ids = [ComponentId; $count];

            fn has_duplicate_types() -> bool {
                // TypeIds are only compared for equality here; they never reach ids, hashes or
                // iteration order.
                let types = [$(std::any::TypeId::of::<$C>()),+];
                types
                    .iter()
                    .enumerate()
                    .any(|(position, ty)| types[position + 1..].contains(ty))
            }

            fn register(registry: &mut ComponentRegistry) -> Self::Ids {
                [$(registry.register::<$C>()),+]
            }

            fn write(self, ids: &Self::Ids, archetype: &mut Archetype) {
                $(archetype.push_component(ids[$index], self.$index);)+
            }
        }

        impl<$($C: Component),+> Bundle for ($($C,)+) {}
    };
}

impl_bundle!(1; (A, 0));
impl_bundle!(2; (A, 0), (B, 1));
impl_bundle!(3; (A, 0), (B, 1), (C, 2));
impl_bundle!(4; (A, 0), (B, 1), (C, 2), (D, 3));
impl_bundle!(5; (A, 0), (B, 1), (C, 2), (D, 3), (E, 4));
impl_bundle!(6; (A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5));
impl_bundle!(7; (A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5), (G, 6));
impl_bundle!(8; (A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5), (G, 6), (H, 7));
