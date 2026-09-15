//! # grimoire_ecs
//!
//! Entity-component-system of the Grimoire engine: archetype storage, queries, commands,
//! resources, stages of an ordered schedule with exclusive and parallel systems (engine
//! ADR-0006), executors, data-parallel queries in fixed blocks, stable world hashing and
//! snapshots.
//!
//! Determinism is the primary design goal (game ADR-0004/0005):
//!
//! - Component ids follow registration order; `TypeId` is only a lookup key in a `BTreeMap`.
//! - Archetypes are created lazily in first-use order; queries visit them in creation order and
//!   rows in dense order. Despawn swap-removes, which changes order reproducibly.
//! - Despawned entity slots are reused oldest-first with a bumped generation.
//! - [`World::stable_hash`] and [`World::snapshot`] cover the complete state.
//! - Parallel stages see an unchanged world and apply their command buffers in list order;
//!   data-parallel blocks have fixed boundaries ([`QUERY_BLOCK_SIZE`]). No state depends on the
//!   [`Executor`] or its thread count; this crate never creates threads.
//!
//! ```
//! use grimoire_core::impl_stable_hash;
//! use grimoire_ecs::{Entity, World};
//!
//! #[derive(Clone, Debug, PartialEq)]
//! struct Pos { x: f32 }
//! impl_stable_hash!(Pos { x });
//!
//! #[derive(Clone)]
//! struct Vel { x: f32 }
//! impl_stable_hash!(Vel { x });
//!
//! let mut world = World::new();
//! let entity = world.spawn((Pos { x: 0.0 }, Vel { x: 2.0 }));
//! for (pos, vel) in world.query_mut::<(&mut Pos, &Vel)>() {
//!     pos.x += vel.x;
//! }
//! let moved: Vec<(Entity, &Pos)> = world.query::<(Entity, &Pos)>().collect();
//! assert_eq!(moved, vec![(entity, &Pos { x: 2.0 })]);
//! ```

mod access;
mod archetype;
mod bundle;
mod command;
mod component;
#[cfg(debug_assertions)]
mod debug_access;
mod entity;
mod error;
mod executor;
mod query;
mod resource;
mod schedule;
mod world;

pub use access::Access;
pub use bundle::Bundle;
pub use command::CommandBuffer;
pub use component::Component;
pub use entity::Entity;
pub use error::EcsError;
pub use executor::{Executor, PermutedExecutor, SequentialExecutor};
pub use query::{
    QUERY_BLOCK_SIZE, Query, QueryBlock, QueryIter, QueryIterMut, ReadOnlyQuery, With, Without,
};
pub use resource::Resource;
pub use schedule::{
    ParallelSystem, Schedule, Stage, StageMode, System, parallel_system_fn, system_fn,
};
pub use world::{World, WorldSnapshot};
