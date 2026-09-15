//! # grimoire_ecs
//!
//! Entity-component-system of the Grimoire engine: archetype storage, queries, commands,
//! resources, an ordered system schedule, stable world hashing and snapshots.
//!
//! Determinism is the primary design goal (game ADR-0004/0005):
//!
//! - Component ids follow registration order; `TypeId` is only a lookup key in a `BTreeMap`.
//! - Archetypes are created lazily in first-use order; queries visit them in creation order and
//!   rows in dense order. Despawn swap-removes, which changes order reproducibly.
//! - Despawned entity slots are reused oldest-first with a bumped generation.
//! - [`World::stable_hash`] and [`World::snapshot`] cover the complete state.
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

mod archetype;
mod bundle;
mod command;
mod component;
mod entity;
mod error;
mod executor;
mod query;
mod resource;
mod schedule;
mod world;

pub use bundle::Bundle;
pub use command::CommandBuffer;
pub use component::Component;
pub use entity::Entity;
pub use error::EcsError;
pub use executor::{Executor, PermutedExecutor, SequentialExecutor};
pub use query::{Query, QueryIter, QueryIterMut, ReadOnlyQuery, With, Without};
pub use resource::Resource;
pub use schedule::{Schedule, System, system_fn};
pub use world::{World, WorldSnapshot};
