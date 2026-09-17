//! `SpatialGrid::overlapping_batch` must be bit-identical to one `overlapping` call per query, in
//! query order, for any executor (contract §14, engine ADR-0006), and a reused `BatchHits` must
//! not carry anything over from an earlier, larger batch.

use grimoire_collide::{
    BatchHits, Capsule, Circle, ColliderKey, CollisionQuery, GridConfig, GridItem, LayerMask,
    Shape, ShapeQuery, SpatialGrid,
};
use grimoire_core::Vec2;
use grimoire_ecs::{Executor, PermutedExecutor, SequentialExecutor};
use proptest::prelude::*;

fn config() -> GridConfig {
    GridConfig::new(Vec2::new(-20.0, -20.0), 2.0, 20, 20)
}

fn mask_strategy() -> impl Strategy<Value = LayerMask> {
    prop::sample::select(vec![
        LayerMask::NONE,
        LayerMask::layer(0),
        LayerMask::layer(1),
        LayerMask::ALL,
    ])
}

fn shape_strategy() -> impl Strategy<Value = Shape> {
    prop_oneof![
        (-30.0f32..30.0, -30.0f32..30.0, 0.0f32..6.0).prop_map(|(x, y, radius)| {
            Shape::Circle(Circle {
                center: Vec2::new(x, y),
                radius,
            })
        }),
        (
            -30.0f32..30.0,
            -30.0f32..30.0,
            -30.0f32..30.0,
            -30.0f32..30.0,
            0.0f32..2.0
        )
            .prop_map(|(ax, ay, bx, by, radius)| Shape::Capsule(Capsule {
                a: Vec2::new(ax, ay),
                b: Vec2::new(bx, by),
                radius,
            })),
    ]
}

fn items_strategy(max_len: usize) -> impl Strategy<Value = Vec<GridItem>> {
    prop::collection::vec((shape_strategy(), mask_strategy()), 0..=max_len).prop_map(|list| {
        list.into_iter()
            .enumerate()
            .map(|(index, (shape, layers))| GridItem {
                key: ColliderKey::pool(index as u32, 0),
                shape,
                layers,
            })
            .collect()
    })
}

fn queries_strategy(max_len: usize) -> impl Strategy<Value = Vec<ShapeQuery>> {
    prop::collection::vec(
        (shape_strategy(), mask_strategy()).prop_map(|(shape, mask)| ShapeQuery { shape, mask }),
        0..=max_len,
    )
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
    fn overlapping_batch_matches_one_query_at_a_time(
        items in items_strategy(150),
        queries in queries_strategy(70),
        earlier in queries_strategy(90),
    ) {
        let mut grid = SpatialGrid::new(config()).expect("valid config");
        grid.rebuild(items);
        let expected: Vec<Vec<_>> = queries
            .iter()
            .map(|query| {
                let mut hits = Vec::new();
                grid.overlapping(&query.shape, query.mask, &mut hits);
                hits
            })
            .collect();

        for executor in executors() {
            // A reused batch that answered a different set of queries before.
            let mut out = BatchHits::default();
            grid.overlapping_batch(executor.as_ref(), &earlier, &mut out);
            grid.overlapping_batch(executor.as_ref(), &queries, &mut out);
            prop_assert_eq!(out.len(), queries.len());
            prop_assert_eq!(out.is_empty(), queries.is_empty());
            for (index, hits) in expected.iter().enumerate() {
                prop_assert_eq!(out.hits(index), hits.as_slice());
            }
        }
    }
}
