//! Golden hash of a fixed scene under a fixed set of queries (contract §14). Frozen once and
//! hardcoded below; a genuine behaviour change (not just a refactor) renews it deliberately.

use grimoire_collide::{
    Capsule, Circle, ColliderKey, CollisionQuery, GrazeRing, GridConfig, GridItem, LayerMask,
    Shape, SpatialGrid,
};
use grimoire_core::{StableHash, StableHasher, Vec2};

/// One fixed, non-trivial scene: a ring of circles of varying radius and layer around the
/// origin, plus a fan of long capsules crossing it, plus a couple of edge cases (a degenerate
/// capsule and a radius-0 circle).
fn build_scene() -> SpatialGrid {
    let config = GridConfig::new(Vec2::new(-50.0, -50.0), 5.0, 20, 20);
    let mut grid = SpatialGrid::new(config).expect("valid config");

    let mut items = Vec::new();
    for i in 0..40u32 {
        let fi = i as f32;
        let x = -45.0 + fi * 2.3;
        let y = -20.0 + ((fi * 1.7) % 40.0);
        items.push(GridItem {
            key: ColliderKey::pool(i, 0),
            shape: Shape::Circle(Circle {
                center: Vec2::new(x, y),
                radius: 0.5 + (i % 4) as f32,
            }),
            layers: LayerMask::layer((i % 5) as u8),
        });
    }
    for i in 0..12u32 {
        let fi = i as f32;
        let ax = -40.0 + fi * 6.0;
        items.push(GridItem {
            key: ColliderKey::pool(1_000 + i, 0),
            shape: Shape::Capsule(Capsule {
                a: Vec2::new(ax, -45.0),
                b: Vec2::new(ax + 3.0, 45.0),
                radius: 0.8,
            }),
            layers: LayerMask::layer(((i % 3) + 5) as u8),
        });
    }
    items.push(GridItem {
        key: ColliderKey::pool(9_000, 0),
        shape: Shape::Capsule(Capsule {
            a: Vec2::new(0.0, 0.0),
            b: Vec2::new(0.0, 0.0),
            radius: 1.5,
        }),
        layers: LayerMask::ALL,
    });
    items.push(GridItem {
        key: ColliderKey::pool(9_001, 0),
        shape: Shape::Circle(Circle {
            center: Vec2::new(5.0, 5.0),
            radius: 0.0,
        }),
        layers: LayerMask::layer(2),
    });

    grid.rebuild(items);
    grid
}

fn overlap_queries() -> Vec<(Shape, LayerMask)> {
    vec![
        (
            Shape::Circle(Circle {
                center: Vec2::ZERO,
                radius: 6.0,
            }),
            LayerMask::ALL,
        ),
        (
            Shape::Circle(Circle {
                center: Vec2::new(10.0, -10.0),
                radius: 4.0,
            }),
            LayerMask::layer(1) | LayerMask::layer(2),
        ),
        (
            Shape::Capsule(Capsule {
                a: Vec2::new(-45.0, -45.0),
                b: Vec2::new(45.0, 45.0),
                radius: 1.0,
            }),
            LayerMask::ALL,
        ),
        (
            Shape::Circle(Circle {
                center: Vec2::new(1_000.0, 1_000.0),
                radius: 1.0,
            }),
            LayerMask::ALL,
        ),
        (
            Shape::Circle(Circle {
                center: Vec2::ZERO,
                radius: 6.0,
            }),
            LayerMask::NONE,
        ),
    ]
}

fn graze_queries() -> Vec<(GrazeRing, LayerMask)> {
    vec![
        (
            GrazeRing {
                center: Vec2::ZERO,
                inner_radius: 2.0,
                outer_radius: 15.0,
            },
            LayerMask::ALL,
        ),
        (
            GrazeRing {
                center: Vec2::new(-20.0, 10.0),
                inner_radius: 0.0,
                outer_radius: 8.0,
            },
            LayerMask::layer(0) | LayerMask::layer(3),
        ),
    ]
}

fn compute_golden_hash() -> u64 {
    let grid = build_scene();
    let mut hasher = StableHasher::new();
    let mut hits = Vec::new();

    for (shape, mask) in overlap_queries() {
        grid.overlapping(&shape, mask, &mut hits);
        hits.stable_hash(&mut hasher);
    }
    for (ring, mask) in graze_queries() {
        grid.graze_ring(&ring, mask, &mut hits);
        hits.stable_hash(&mut hasher);
    }

    hasher.finish()
}

/// Frozen by running this test once (`cargo test -p grimoire_collide --all-features -- golden`)
/// and hardcoding the printed/asserted value.
const GOLDEN_QUERY_HASH: u64 = 0x139c_dc97_949f_6eb7;

#[test]
fn golden_query_hash_is_stable() {
    let hash = compute_golden_hash();
    assert_eq!(
        hash, GOLDEN_QUERY_HASH,
        "golden hash changed: {hash:#018x} (update GOLDEN_QUERY_HASH deliberately if this is an \
         intentional behaviour change)"
    );
}
