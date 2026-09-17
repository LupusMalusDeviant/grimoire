//! Runs the `grimoire_collide::conformance` suite (contract §2 rule 12, §14) generically against
//! every `CollisionQuery` implementation this crate ships: `NullCollision`, `BruteForceQuery` and
//! `SpatialGrid`, each built by its own producer over the same object set.
//!
//! Behind the non-default feature `conformance`, like the suite itself. CI runs it in the step
//! "Test (grimoire_collide, feature conformance)"; a plain `cargo test --workspace` compiles this
//! file to an empty test binary.

#![cfg(feature = "conformance")]

use grimoire_collide::{
    BruteForceQuery, Capsule, Circle, ColliderKey, CollisionQuery, GrazeRing, GridConfig, GridItem,
    LayerMask, NullCollision, Shape, SpatialGrid, conformance,
};
use grimoire_core::Vec2;

/// The object set every implementation is checked against: overlapping circles on different
/// layers, a long capsule through the probe, an object on no layer, and one outside the grid.
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
        GridItem {
            key: ColliderKey::pool(3, 0),
            shape: Shape::Circle(Circle {
                center: Vec2::new(-3.0, -3.0),
                radius: 0.2,
            }),
            layers: LayerMask::NONE,
        },
        GridItem {
            key: ColliderKey::pool(4, 0),
            shape: Shape::Circle(Circle {
                center: Vec2::new(40.0, 2.0),
                radius: 1.0,
            }),
            layers: LayerMask::ALL,
        },
    ]
}

/// Probes that find several objects in a real implementation, including through the ring.
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

/// Runs the suite against `query`, generically over the implementation.
fn check(query: &impl CollisionQuery) {
    let (shape, ring) = probe();
    conformance::collision_query(query, &shape, &ring);
}

#[test]
fn null_collision_is_conformant() {
    check(&NullCollision);
}

#[test]
fn brute_force_query_is_conformant() {
    let items = sample_items();
    check(&BruteForceQuery::new(&items));
}

#[test]
fn spatial_grid_is_conformant() {
    let mut grid = SpatialGrid::new(GridConfig::new(Vec2::new(-10.0, -10.0), 1.0, 20, 20))
        .expect("valid config");
    grid.rebuild(sample_items());
    check(&grid);
}
