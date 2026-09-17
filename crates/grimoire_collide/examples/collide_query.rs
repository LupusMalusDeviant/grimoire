//! Console example of `grimoire_collide` v0 (plan 0002 WP6.5, contract §14): builds a small
//! bullet curtain in a uniform `SpatialGrid`, asks it which bullets touch each enemy and which
//! ones graze the player, and checks every answer against `BruteForceQuery`.
//!
//! ```text
//! cargo run -p grimoire_collide --example collide_query
//! ```
//!
//! No window, no timing: the budget proof lives in `grimoire_bench` (`collide_uniform`,
//! `collide_cluster`). CI only builds this example.

use grimoire_collide::{
    BatchHits, BruteForceQuery, Capsule, Circle, ColliderKey, CollisionQuery, GrazeRing,
    GridConfig, GridItem, LayerMask, Shape, ShapeQuery, SpatialGrid,
};
use grimoire_core::Vec2;
use grimoire_ecs::SequentialExecutor;

/// The game owns the meaning of the 32 layer bits; this example uses two.
const BULLETS: LayerMask = LayerMask::layer(0);
const ENEMIES: LayerMask = LayerMask::layer(1);

/// Rings of bullets around the origin: `rings` rings of `per_ring` bullets, 1.5 units apart.
fn bullet_curtain(rings: u32, per_ring: u32) -> Vec<GridItem> {
    let mut items = Vec::new();
    for ring in 0..rings {
        let radius = 3.0 + ring as f32 * 1.5;
        for step in 0..per_ring {
            // Positions from a unit square walk instead of trigonometry: the collision crate's
            // determinism lints forbid platform `sin`/`cos`, and a square curtain reads just as
            // well in the output.
            let t = step as f32 / per_ring as f32 * 4.0;
            let (x, y) = match t as u32 {
                0 => (-1.0 + 2.0 * t.fract(), -1.0),
                1 => (1.0, -1.0 + 2.0 * t.fract()),
                2 => (1.0 - 2.0 * t.fract(), 1.0),
                _ => (-1.0, 1.0 - 2.0 * t.fract()),
            };
            items.push(GridItem {
                key: ColliderKey::pool(ring * per_ring + step, 0),
                shape: Shape::Circle(Circle {
                    center: Vec2::new(x * radius, y * radius),
                    radius: 0.3,
                }),
                layers: BULLETS,
            });
        }
    }
    items
}

/// Three dummy enemies: two round, one long (a capsule).
fn enemies() -> Vec<(&'static str, Shape)> {
    vec![
        (
            "imp",
            Shape::Circle(Circle {
                center: Vec2::new(6.0, 0.0),
                radius: 1.0,
            }),
        ),
        (
            "wraith",
            Shape::Circle(Circle {
                center: Vec2::new(-9.0, 9.0),
                radius: 1.5,
            }),
        ),
        (
            "serpent",
            Shape::Capsule(Capsule {
                a: Vec2::new(-12.0, -12.0),
                b: Vec2::new(-4.0, -12.0),
                radius: 0.8,
            }),
        ),
    ]
}

fn keys(hits: &[grimoire_collide::Hit]) -> Vec<u32> {
    hits.iter().map(|hit| hit.key.index).collect()
}

fn main() {
    let curtain = bullet_curtain(8, 48);
    let mut grid = SpatialGrid::new(GridConfig::new(Vec2::new(-32.0, -32.0), 4.0, 16, 16))
        .expect("a valid grid configuration");
    grid.rebuild_par(&SequentialExecutor, curtain.iter().copied());
    let reference = BruteForceQuery::new(&curtain);
    println!(
        "{} bullets in a {}x{} grid of {}-unit cells",
        grid.len(),
        grid.config().columns,
        grid.config().rows,
        grid.config().cell_size
    );

    // One query per enemy, answered together; each answer equals a single query and the
    // brute-force reference, ascending by collider key.
    let enemies = enemies();
    let queries: Vec<ShapeQuery> = enemies
        .iter()
        .map(|&(_, shape)| ShapeQuery {
            shape,
            mask: BULLETS,
        })
        .collect();
    let mut batch = BatchHits::default();
    grid.overlapping_batch(&SequentialExecutor, &queries, &mut batch);
    let mut expected = Vec::new();
    for (index, (name, shape)) in enemies.iter().enumerate() {
        reference.overlapping(shape, BULLETS, &mut expected);
        assert_eq!(batch.hits(index), expected.as_slice());
        println!(
            "{name:>8} touches {:>2} bullets: {:?}",
            batch.hits(index).len(),
            keys(batch.hits(index))
        );
    }

    // Enemy layers never match the bullet mask.
    let mut hits = Vec::new();
    grid.overlapping(&enemies[0].1, ENEMIES, &mut hits);
    assert!(hits.is_empty());

    // Graze: bullets that come within 2 units of the player without touching its core.
    let ring = GrazeRing {
        center: Vec2::new(0.0, -4.0),
        inner_radius: 0.4,
        outer_radius: 2.0,
    };
    grid.graze_ring(&ring, BULLETS, &mut hits);
    reference.graze_ring(&ring, BULLETS, &mut expected);
    assert_eq!(hits, expected);
    println!(
        "  player grazes {:>2} bullets: {:?}",
        hits.len(),
        keys(&hits)
    );
}
