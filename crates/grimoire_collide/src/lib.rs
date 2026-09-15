//! # grimoire_collide
//!
//! 2D collision for the Grimoire engine — Kollision v0 (contract §14): circle and capsule
//! shapes, layer masks, an exact overlap test, a uniform spatial-grid broadphase and a
//! graze-ring query, all without gameplay effect (a budget proof only; hit responses, the "once
//! per bullet" graze economy and the parry arc follow in P2).
//!
//! - [`Shape`], [`Circle`], [`Capsule`], [`Aabb`], [`overlaps`]: shapes and the exact,
//!   allocation-free overlap test (no square root, no trigonometry).
//! - [`LayerMask`]: which colliders can find each other.
//! - [`ColliderKey`], [`Collider`], [`GridItem`], [`Hit`]: collider identity and query results.
//! - [`CollisionQuery`]: the trait consumers query against (contract §2a), with
//!   [`NullCollision`] (headless), [`BruteForceQuery`] (reference) and [`SpatialGrid`]
//!   (accelerated, simulation state) implementing it.
//! - [`GrazeRing`], [`ShapeQuery`], [`BatchHits`]: the graze-ring query and the data-parallel
//!   batch query.
//!
//! This crate belongs to the determinism set (contract §3): no `HashMap`/`HashSet`, no
//! wall-clock time, no threads of its own — data-parallel work runs through
//! [`grimoire_ecs::run_blocks`] behind the caller-supplied [`grimoire_ecs::Executor`].
//!
//! ```
//! use grimoire_collide::{
//!     BruteForceQuery, Circle, ColliderKey, CollisionQuery, GridItem, Hit, LayerMask, Shape,
//! };
//! use grimoire_core::Vec2;
//!
//! let items = [GridItem {
//!     key: ColliderKey::pool(0, 0),
//!     shape: Shape::Circle(Circle { center: Vec2::ZERO, radius: 1.0 }),
//!     layers: LayerMask::layer(0),
//! }];
//! let query = BruteForceQuery::new(&items);
//! let mut hits = Vec::new();
//! let probe = Shape::Circle(Circle { center: Vec2::ZERO, radius: 0.1 });
//! query.overlapping(&probe, LayerMask::ALL, &mut hits);
//! assert_eq!(hits, vec![Hit { key: items[0].key, layers: items[0].layers }]);
//! ```

mod collider;
#[cfg(feature = "conformance")]
pub mod conformance;
mod grid;
mod key;
mod layers;
mod query;
mod shapes;

pub use collider::{Collider, GrazeRing, GridItem, Hit, ShapeQuery};
pub use grid::{BatchHits, CollideError, GridConfig, MAX_GRID_CELLS, SpatialGrid};
pub use key::{ColliderKey, SOURCE_ENTITY, SOURCE_POOL};
pub use layers::LayerMask;
pub use query::{BruteForceQuery, CollisionQuery, NullCollision};
pub use shapes::{Aabb, Capsule, Circle, MAX_COORD, Shape, overlaps};

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_resource<R: grimoire_ecs::Resource>() {}
    fn assert_component<C: grimoire_ecs::Component>() {}

    #[test]
    fn bounds() {
        assert_resource::<SpatialGrid>();
        assert_component::<Collider>();
        let _: Option<&dyn CollisionQuery> = None;
        assert_eq!(LayerMask::default(), LayerMask::NONE);
    }
}
