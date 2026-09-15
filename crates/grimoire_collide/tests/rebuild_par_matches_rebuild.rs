//! `SpatialGrid::rebuild_par` must be byte-identical to `SpatialGrid::rebuild` for any executor
//! (contract §14, engine ADR-0006): same hash, same query results.

use grimoire_collide::{
    Circle, ColliderKey, CollisionQuery, GridConfig, GridItem, LayerMask, Shape, SpatialGrid,
};
use grimoire_core::{Vec2, hash_of};
use grimoire_ecs::{Executor, PermutedExecutor, SequentialExecutor};
use proptest::prelude::*;

fn config() -> GridConfig {
    GridConfig::new(Vec2::new(-20.0, -20.0), 2.0, 20, 20)
}

fn items_strategy(max_len: usize) -> impl Strategy<Value = Vec<GridItem>> {
    prop::collection::vec(
        (-30.0f32..30.0, -30.0f32..30.0, 0.0f32..4.0, 0u32..3),
        0..=max_len,
    )
    .prop_map(|list| {
        list.into_iter()
            .enumerate()
            .map(|(index, (x, y, radius, layer))| GridItem {
                key: ColliderKey::pool(index as u32, 0),
                shape: Shape::Circle(Circle {
                    center: Vec2::new(x, y),
                    radius,
                }),
                layers: LayerMask::layer(layer as u8),
            })
            .collect()
    })
}

fn executors() -> Vec<Box<dyn Executor>> {
    vec![
        Box::new(SequentialExecutor),
        Box::new(PermutedExecutor::new(1)),
        Box::new(PermutedExecutor::new(2)),
        Box::new(PermutedExecutor::new(3)),
        Box::new(PermutedExecutor::reversed()),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn rebuild_par_matches_rebuild_for_every_executor(items in items_strategy(200)) {
        let mut sequential = SpatialGrid::new(config()).expect("valid config");
        sequential.rebuild(items.iter().copied());
        let sequential_hash = hash_of(&sequential);

        for executor in executors() {
            let mut parallel = SpatialGrid::new(config()).expect("valid config");
            parallel.rebuild_par(executor.as_ref(), items.iter().copied());

            prop_assert_eq!(hash_of(&parallel), sequential_hash);
            prop_assert_eq!(parallel.items(), sequential.items());

            // Byte-identical query behaviour, not just the same hash: probe a few shapes.
            for probe in [
                Shape::Circle(Circle { center: Vec2::ZERO, radius: 3.0 }),
                Shape::Circle(Circle { center: Vec2::new(15.0, 15.0), radius: 5.0 }),
                Shape::Circle(Circle { center: Vec2::new(-100.0, -100.0), radius: 2.0 }),
            ] {
                let mut expected = Vec::new();
                let mut actual = Vec::new();
                sequential.overlapping(&probe, LayerMask::ALL, &mut expected);
                parallel.overlapping(&probe, LayerMask::ALL, &mut actual);
                prop_assert_eq!(actual, expected);
            }
        }
    }
}

/// A panic partway through `rebuild_par` (here: the debug-only duplicate-key check) must not
/// leave the grid half-built — the contract requires it to end up empty (§14). `grimoire_ecs`'s
/// own test suite already covers that `run_blocks` never swallows a block panic and resumes the
/// lowest-index one; this test only checks the grid's state on the near side of that call.
#[test]
fn rebuild_par_leaves_the_grid_empty_after_a_panic() {
    let items: Vec<GridItem> = (0..10)
        .map(|index| GridItem {
            key: ColliderKey::pool(index, 0),
            shape: Shape::Circle(Circle {
                center: Vec2::new(f32::from(index as i16), 0.0),
                radius: 1.0,
            }),
            layers: LayerMask::ALL,
        })
        .collect();
    let mut grid = SpatialGrid::new(config()).expect("valid config");
    grid.rebuild(items.iter().copied());
    assert!(!grid.items().is_empty());

    // A duplicate key makes the debug-only validation panic before any block runs; either way
    // (debug or release) the grid ends up cleared afterwards.
    let mut duplicated = items.clone();
    duplicated.push(items[0]);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        grid.rebuild_par(&SequentialExecutor, duplicated);
    }));
    if cfg!(debug_assertions) {
        assert!(result.is_err());
    }
    assert!(
        grid.items().is_empty(),
        "grid must be empty after a failed rebuild_par"
    );
}
