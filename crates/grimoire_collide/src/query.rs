//! The [`CollisionQuery`] trait (contract §2a) and its null and reference implementations
//! (contract §14).

use crate::GrazeRing;
use crate::collider::{GridItem, Hit};
use crate::layers::LayerMask;
use crate::shapes::{Circle, Shape, overlaps};

/// Object-safe query surface over a broadphase (contract §2a, §14): find colliders overlapping a
/// shape, or lying in a graze ring, filtered by [`LayerMask`].
///
/// Every query first clears `out`, then fills it ascending by [`crate::ColliderKey`] with no
/// duplicates — an object present in multiple grid cells appears once. The order never depends on
/// insertion order, cell size, grid bounds, executor or thread count, so [`BruteForceQuery`] and
/// [`crate::SpatialGrid`] return the same `out` for the same object set; [`NullCollision`] always
/// returns an empty `out`.
pub trait CollisionQuery: Send + Sync {
    /// Number of colliders entered into this query.
    fn len(&self) -> usize;

    /// Whether no collider is entered.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Colliders whose shape overlaps `shape` and whose layers intersect `mask`.
    fn overlapping(&self, shape: &Shape, mask: LayerMask, out: &mut Vec<Hit>);

    /// Colliders overlapping the outer circle of `ring` but not its inner circle, filtered by
    /// `mask`.
    fn graze_ring(&self, ring: &GrazeRing, mask: LayerMask, out: &mut Vec<Hit>);
}

/// Null implementation: never has any colliders; every query only clears `out`.
#[derive(Clone, Copy, Default, Debug)]
pub struct NullCollision;

impl CollisionQuery for NullCollision {
    fn len(&self) -> usize {
        0
    }

    fn overlapping(&self, _shape: &Shape, _mask: LayerMask, out: &mut Vec<Hit>) {
        out.clear();
    }

    fn graze_ring(&self, _ring: &GrazeRing, _mask: LayerMask, out: &mut Vec<Hit>) {
        out.clear();
    }
}

/// Reference implementation: checks every object exactly. Used as the conformance and property
/// baseline for [`crate::SpatialGrid`].
#[derive(Debug)]
pub struct BruteForceQuery<'a> {
    items: &'a [GridItem],
}

impl<'a> BruteForceQuery<'a> {
    /// Wraps `items` for brute-force queries.
    #[must_use]
    pub fn new(items: &'a [GridItem]) -> Self {
        Self { items }
    }
}

impl CollisionQuery for BruteForceQuery<'_> {
    fn len(&self) -> usize {
        self.items.len()
    }

    fn overlapping(&self, shape: &Shape, mask: LayerMask, out: &mut Vec<Hit>) {
        collect_overlap_hits(shape, mask, self.items.iter(), out);
    }

    fn graze_ring(&self, ring: &GrazeRing, mask: LayerMask, out: &mut Vec<Hit>) {
        collect_graze_hits(ring, mask, self.items.iter(), out);
    }
}

fn hit_of(item: &GridItem) -> Hit {
    Hit {
        key: item.key,
        layers: item.layers,
    }
}

/// Whether `ring` satisfies `0 <= inner_radius <= outer_radius` with both finite (contract §14).
///
/// Not part of the public API: the contract only exposes [`Shape::is_valid`] as a public validity
/// check, so this stays an internal helper shared by the query implementations.
pub(crate) fn graze_ring_is_valid(ring: &GrazeRing) -> bool {
    ring.inner_radius.is_finite()
        && ring.outer_radius.is_finite()
        && ring.inner_radius >= 0.0
        && ring.inner_radius <= ring.outer_radius
}

/// The outer and inner circles of `ring`, in that order.
pub(crate) fn ring_shapes(ring: &GrazeRing) -> (Shape, Shape) {
    (
        Shape::Circle(Circle {
            center: ring.center,
            radius: ring.outer_radius,
        }),
        Shape::Circle(Circle {
            center: ring.center,
            radius: ring.inner_radius,
        }),
    )
}

/// Shared `overlapping` filter for [`BruteForceQuery`] and [`crate::SpatialGrid`]: clears `out`,
/// fills it with every item in `items` whose layers intersect `mask` and whose shape overlaps
/// `shape`, then sorts ascending and dedupes (contract §14).
pub(crate) fn collect_overlap_hits<'a>(
    shape: &Shape,
    mask: LayerMask,
    items: impl Iterator<Item = &'a GridItem>,
    out: &mut Vec<Hit>,
) {
    out.clear();
    out.extend(
        items
            .filter(|item| item.layers.intersects(mask) && overlaps(&item.shape, shape))
            .map(hit_of),
    );
    out.sort_unstable();
    out.dedup();
}

/// Shared `graze_ring` filter for [`BruteForceQuery`] and [`crate::SpatialGrid`] (contract §14):
/// clears `out`; if `ring` is invalid, leaves it empty (debug builds additionally panic);
/// otherwise fills it with every item in `items` whose layers intersect `mask`, which overlaps
/// the outer circle and does not overlap the inner circle, then sorts ascending and dedupes.
pub(crate) fn collect_graze_hits<'a>(
    ring: &GrazeRing,
    mask: LayerMask,
    items: impl Iterator<Item = &'a GridItem>,
    out: &mut Vec<Hit>,
) {
    out.clear();
    let valid = graze_ring_is_valid(ring);
    debug_assert!(valid, "invalid graze ring {ring:?}");
    if !valid {
        return;
    }
    let (outer, inner) = ring_shapes(ring);
    out.extend(
        items
            .filter(|item| {
                item.layers.intersects(mask)
                    && overlaps(&item.shape, &outer)
                    && !overlaps(&item.shape, &inner)
            })
            .map(hit_of),
    );
    out.sort_unstable();
    out.dedup();
}

#[cfg(test)]
mod tests {
    use grimoire_core::Vec2;

    use super::*;
    use crate::key::ColliderKey;

    fn item(index: u32, x: f32, layers: LayerMask) -> GridItem {
        GridItem {
            key: ColliderKey::pool(index, 0),
            shape: Shape::Circle(Circle {
                center: Vec2::new(x, 0.0),
                radius: 1.0,
            }),
            layers,
        }
    }

    #[test]
    fn null_collision_is_always_empty() {
        let null = NullCollision;
        assert_eq!(null.len(), 0);
        assert!(null.is_empty());
        let mut out = vec![Hit {
            key: ColliderKey::pool(0, 0),
            layers: LayerMask::ALL,
        }];
        null.overlapping(
            &Shape::Circle(Circle {
                center: Vec2::ZERO,
                radius: 1.0,
            }),
            LayerMask::ALL,
            &mut out,
        );
        assert!(out.is_empty());
        let mut out = vec![Hit {
            key: ColliderKey::pool(0, 0),
            layers: LayerMask::ALL,
        }];
        null.graze_ring(
            &GrazeRing {
                center: Vec2::ZERO,
                inner_radius: 0.0,
                outer_radius: 1.0,
            },
            LayerMask::ALL,
            &mut out,
        );
        assert!(out.is_empty());
    }

    #[test]
    fn brute_force_finds_overlapping_items_sorted_without_duplicates() {
        let items = [
            item(2, 5.0, LayerMask::layer(0)),
            item(0, 0.0, LayerMask::layer(0)),
            item(1, 0.5, LayerMask::layer(1)),
        ];
        let query = BruteForceQuery::new(&items);
        assert_eq!(query.len(), 3);
        let mut out = Vec::new();
        query.overlapping(
            &Shape::Circle(Circle {
                center: Vec2::ZERO,
                radius: 1.0,
            }),
            LayerMask::ALL,
            &mut out,
        );
        assert_eq!(
            out,
            vec![
                Hit {
                    key: items[1].key,
                    layers: LayerMask::layer(0)
                },
                Hit {
                    key: items[2].key,
                    layers: LayerMask::layer(1)
                },
            ]
        );
    }

    #[test]
    fn brute_force_layer_none_finds_nothing_and_layer_none_items_are_never_found() {
        let items = [
            item(0, 0.0, LayerMask::NONE),
            item(1, 0.1, LayerMask::layer(0)),
        ];
        let query = BruteForceQuery::new(&items);
        let mut out = Vec::new();
        query.overlapping(
            &Shape::Circle(Circle {
                center: Vec2::ZERO,
                radius: 1.0,
            }),
            LayerMask::NONE,
            &mut out,
        );
        assert!(out.is_empty());
        query.overlapping(
            &Shape::Circle(Circle {
                center: Vec2::ZERO,
                radius: 1.0,
            }),
            LayerMask::ALL,
            &mut out,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].key, items[1].key);
    }

    /// Small-radius item at `(x, 0)`, so the graze test below can reason about the item's
    /// distance from the ring center without its own radius blurring the inner/outer boundary.
    fn small_item(index: u32, x: f32, layers: LayerMask) -> GridItem {
        GridItem {
            key: ColliderKey::pool(index, 0),
            shape: Shape::Circle(Circle {
                center: Vec2::new(x, 0.0),
                radius: 0.1,
            }),
            layers,
        }
    }

    #[test]
    fn brute_force_graze_ring_hits_between_the_two_circles() {
        let items = [
            small_item(0, 0.4, LayerMask::ALL), // overlaps inner radius: excluded
            small_item(1, 2.0, LayerMask::ALL), // between the circles: included
            small_item(2, 10.0, LayerMask::ALL), // outside outer radius: excluded
        ];
        let query = BruteForceQuery::new(&items);
        let mut out = Vec::new();
        query.graze_ring(
            &GrazeRing {
                center: Vec2::ZERO,
                inner_radius: 1.0,
                outer_radius: 3.0,
            },
            LayerMask::ALL,
            &mut out,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].key, items[1].key);
    }

    #[test]
    fn invalid_graze_ring_yields_empty_output() {
        let items = [item(0, 0.0, LayerMask::ALL)];
        let query = BruteForceQuery::new(&items);
        let mut out = Vec::new();
        // inner > outer is invalid.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            query.graze_ring(
                &GrazeRing {
                    center: Vec2::ZERO,
                    inner_radius: 3.0,
                    outer_radius: 1.0,
                },
                LayerMask::ALL,
                &mut out,
            );
        }));
        if cfg!(debug_assertions) {
            assert!(result.is_err(), "debug build must panic on an invalid ring");
        } else {
            assert!(result.is_ok());
            assert!(out.is_empty());
        }
    }

    #[cfg(all(test, feature = "conformance"))]
    mod conformance_tests {
        use grimoire_core::Vec2;

        use super::*;
        use crate::shapes::Capsule;

        fn sample_items() -> Vec<GridItem> {
            vec![
                GridItem {
                    key: ColliderKey::pool(0, 0),
                    shape: Shape::Circle(Circle {
                        center: Vec2::new(0.0, 0.0),
                        radius: 1.0,
                    }),
                    layers: LayerMask::layer(0),
                },
                GridItem {
                    key: ColliderKey::pool(1, 0),
                    shape: Shape::Circle(Circle {
                        center: Vec2::new(0.5, 0.0),
                        radius: 1.0,
                    }),
                    layers: LayerMask::layer(1),
                },
                GridItem {
                    key: ColliderKey::pool(2, 0),
                    shape: Shape::Capsule(Capsule {
                        a: Vec2::new(-2.0, -2.0),
                        b: Vec2::new(2.0, 2.0),
                        radius: 0.5,
                    }),
                    layers: LayerMask::ALL,
                },
            ]
        }

        fn probe() -> (Shape, GrazeRing) {
            (
                Shape::Circle(Circle {
                    center: Vec2::ZERO,
                    radius: 1.5,
                }),
                GrazeRing {
                    center: Vec2::ZERO,
                    inner_radius: 0.2,
                    outer_radius: 3.0,
                },
            )
        }

        #[test]
        fn null_collision_is_conformant() {
            let (shape, ring) = probe();
            crate::conformance::collision_query(&NullCollision, &shape, &ring);
        }

        #[test]
        fn brute_force_query_is_conformant() {
            let items = sample_items();
            let query = BruteForceQuery::new(&items);
            let (shape, ring) = probe();
            crate::conformance::collision_query(&query, &shape, &ring);
        }
    }
}
