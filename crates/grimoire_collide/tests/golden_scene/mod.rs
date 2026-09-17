//! The golden query scene of contract §14: a fixed object set under a fixed set of overlap and
//! graze-ring queries, hashed hit list by hit list.
//!
//! Shared by `tests/golden_hash.rs` (sequential `rebuild` and single queries) and by
//! `grimoire_exec/tests/hash_gate.rs` (`rebuild_par`, `overlapping_batch` and `graze_ring` on
//! thread pools with 1, 2 and N threads), which includes this file via `#[path]` (contract §14,
//! PO decision V-1). Both must reproduce [`GOLDEN_QUERY_HASH`]. Only the public API of
//! `grimoire_collide` is used.

#![allow(dead_code)] // each including test binary uses a different subset.

use grimoire_collide::{
    BatchHits, Capsule, Circle, ColliderKey, CollisionQuery, GrazeRing, GridConfig, GridItem,
    LayerMask, Shape, ShapeQuery, SpatialGrid,
};
use grimoire_core::{StableHash, StableHasher, Vec2};
use grimoire_ecs::Executor;

/// Frozen when the scene was introduced (WP1.3); a genuine behaviour change (not a refactor)
/// renews it deliberately.
pub const GOLDEN_QUERY_HASH: u64 = 0x139c_dc97_949f_6eb7;

/// One fixed, non-trivial scene: a ring of circles of varying radius and layer around the
/// origin, plus a fan of long capsules crossing it, plus a couple of edge cases (a degenerate
/// capsule and a radius-0 circle).
pub fn build_scene() -> SpatialGrid {
    let mut grid = SpatialGrid::new(config()).expect("valid config");
    grid.rebuild(items());
    grid
}

/// The scene's grid geometry.
pub fn config() -> GridConfig {
    GridConfig::new(Vec2::new(-50.0, -50.0), 5.0, 20, 20)
}

/// The scene's objects, in insertion order.
pub fn items() -> Vec<GridItem> {
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

    items
}

pub fn overlap_queries() -> Vec<(Shape, LayerMask)> {
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

pub fn graze_queries() -> Vec<(GrazeRing, LayerMask)> {
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

/// Hash of the scene's query results through `rebuild` and single `overlapping`/`graze_ring`
/// calls: every overlap query's hit list in order, then every graze query's.
pub fn sequential_query_hash() -> u64 {
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

/// The same hash through the data-parallel path on `executor`: `rebuild_par`, then all overlap
/// queries at once through `overlapping_batch`, then the graze queries. Must equal
/// [`sequential_query_hash`] for every executor (contract §14).
pub fn parallel_query_hash(executor: &dyn Executor) -> u64 {
    let mut grid = SpatialGrid::new(config()).expect("valid config");
    grid.rebuild_par(executor, items());
    let queries: Vec<ShapeQuery> = overlap_queries()
        .into_iter()
        .map(|(shape, mask)| ShapeQuery { shape, mask })
        .collect();
    let mut batch = BatchHits::default();
    grid.overlapping_batch(executor, &queries, &mut batch);
    let mut hasher = StableHasher::new();
    for index in 0..batch.len() {
        batch.hits(index).stable_hash(&mut hasher);
    }
    let mut hits = Vec::new();
    for (ring, mask) in graze_queries() {
        grid.graze_ring(&ring, mask, &mut hits);
        hits.stable_hash(&mut hasher);
    }
    hasher.finish()
}

// ---- The block scene: large enough that both data-parallel paths use several blocks -------------

/// Frozen when the block scene was introduced (plan 0002 WP6.5); a genuine behaviour change (not
/// a refactor) renews it deliberately.
pub const GOLDEN_BLOCK_QUERY_HASH: u64 = 0xbed4_7105_bde1_ecf2;

/// Objects of the block scene: more than twice `grimoire_ecs::QUERY_BLOCK_SIZE`, so `rebuild_par`
/// splits them into three blocks.
pub const BLOCK_SCENE_ITEMS: u32 = 3_000;

/// Overlap queries of the block scene: several blocks of `overlapping_batch`.
pub const BLOCK_SCENE_QUERIES: u32 = 64;

/// The block scene's grid geometry.
pub fn block_config() -> GridConfig {
    GridConfig::new(Vec2::new(-50.0, -50.0), 2.5, 40, 40)
}

/// The block scene's objects: circles and every tenth a short capsule, on integer-derived
/// positions over the grid and a little beyond it, on four layers, with mixed generations.
pub fn block_items() -> Vec<GridItem> {
    (0..BLOCK_SCENE_ITEMS)
        .map(|i| {
            let x = (i * 37 % 440) as f32 * 0.25 - 55.0;
            let y = (i * 91 % 440) as f32 * 0.25 - 55.0;
            let radius = 0.2 + (i % 7) as f32 * 0.15;
            let shape = if i % 10 == 0 {
                Shape::Capsule(Capsule {
                    a: Vec2::new(x, y),
                    b: Vec2::new(x + 1.5, y - 0.75),
                    radius: radius * 0.5,
                })
            } else {
                Shape::Circle(Circle {
                    center: Vec2::new(x, y),
                    radius,
                })
            };
            GridItem {
                key: ColliderKey::pool(i, i % 3),
                shape,
                layers: LayerMask::layer((i % 4) as u8),
            }
        })
        .collect()
}

/// The block scene's overlap queries: circles and capsules of different sizes across the grid,
/// with changing masks.
pub fn block_queries() -> Vec<ShapeQuery> {
    (0..BLOCK_SCENE_QUERIES)
        .map(|i| {
            let x = (i * 13 % 32) as f32 * 3.0 - 48.0;
            let y = (i * 7 % 32) as f32 * 3.0 - 48.0;
            let shape = if i % 3 == 0 {
                Shape::Capsule(Capsule {
                    a: Vec2::new(x, y),
                    b: Vec2::new(x + 8.0, y + 3.0),
                    radius: 1.5,
                })
            } else {
                Shape::Circle(Circle {
                    center: Vec2::new(x, y),
                    radius: 2.0 + (i % 5) as f32 * 1.5,
                })
            };
            let mask = match i % 4 {
                0 | 1 => LayerMask::ALL,
                2 => LayerMask::layer(0) | LayerMask::layer(2),
                _ => LayerMask::layer(1) | LayerMask::layer(3),
            };
            ShapeQuery { shape, mask }
        })
        .collect()
}

/// Graze rings of the block scene.
pub fn block_graze_queries() -> Vec<(GrazeRing, LayerMask)> {
    vec![
        (
            GrazeRing {
                center: Vec2::ZERO,
                inner_radius: 3.0,
                outer_radius: 12.0,
            },
            LayerMask::ALL,
        ),
        (
            GrazeRing {
                center: Vec2::new(20.0, -15.0),
                inner_radius: 0.0,
                outer_radius: 6.0,
            },
            LayerMask::layer(1) | LayerMask::layer(2),
        ),
    ]
}

/// Hash of the block scene through `rebuild` and single queries.
pub fn block_sequential_query_hash() -> u64 {
    let mut grid = SpatialGrid::new(block_config()).expect("valid config");
    grid.rebuild(block_items());
    let mut hasher = StableHasher::new();
    let mut hits = Vec::new();
    for query in block_queries() {
        grid.overlapping(&query.shape, query.mask, &mut hits);
        hits.stable_hash(&mut hasher);
    }
    for (ring, mask) in block_graze_queries() {
        grid.graze_ring(&ring, mask, &mut hits);
        hits.stable_hash(&mut hasher);
    }
    hasher.finish()
}

/// Hash of the block scene through `rebuild_par` and `overlapping_batch` on `executor`; must
/// equal [`block_sequential_query_hash`] for every executor.
pub fn block_parallel_query_hash(executor: &dyn Executor) -> u64 {
    let mut grid = SpatialGrid::new(block_config()).expect("valid config");
    grid.rebuild_par(executor, block_items());
    let mut batch = BatchHits::default();
    grid.overlapping_batch(executor, &block_queries(), &mut batch);
    let mut hasher = StableHasher::new();
    for index in 0..batch.len() {
        batch.hits(index).stable_hash(&mut hasher);
    }
    let mut hits = Vec::new();
    for (ring, mask) in block_graze_queries() {
        grid.graze_ring(&ring, mask, &mut hits);
        hits.stable_hash(&mut hasher);
    }
    hasher.finish()
}
