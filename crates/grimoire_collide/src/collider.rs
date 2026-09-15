//! Collider data attached to entities, broadphase entries, query results and query descriptors
//! (contract §14).

use grimoire_core::{Vec2, impl_stable_hash};

use crate::key::ColliderKey;
use crate::layers::LayerMask;
use crate::shapes::Shape;

/// Component: a shape in world coordinates plus the layers it belongs to.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Collider {
    /// Shape in world coordinates.
    pub shape: Shape,
    /// Layers this collider belongs to.
    pub layers: LayerMask,
}
impl_stable_hash!(Collider { shape, layers });

/// One broadphase entry: a collider's identity, shape and layers.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct GridItem {
    /// Stable identity of the collider.
    pub key: ColliderKey,
    /// Shape in world coordinates.
    pub shape: Shape,
    /// Layers this collider belongs to.
    pub layers: LayerMask,
}
impl_stable_hash!(GridItem { key, shape, layers });

/// One query result: which collider was found and its layers.
///
/// Ordering compares `key`, then `layers` (contract §14) — query results are sorted ascending by
/// this order with no duplicates.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Hit {
    /// Identity of the collider that was found.
    pub key: ColliderKey,
    /// Layers of the collider that was found.
    pub layers: LayerMask,
}
impl_stable_hash!(Hit { key, layers });

/// A ring query around `center`: a collider hits if it overlaps the outer circle but not the
/// inner one (contract §14).
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct GrazeRing {
    /// Center of both circles.
    pub center: Vec2,
    /// Radius of the circle a hit must not overlap.
    pub inner_radius: f32,
    /// Radius of the circle a hit must overlap.
    pub outer_radius: f32,
}
impl_stable_hash!(GrazeRing {
    center,
    inner_radius,
    outer_radius
});

/// One shape query for [`crate::SpatialGrid::overlapping_batch`].
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ShapeQuery {
    /// Shape to query with.
    pub shape: Shape,
    /// Layer mask to query with.
    pub mask: LayerMask,
}
