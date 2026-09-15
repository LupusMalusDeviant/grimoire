//! Property tests: [`SpatialGrid`] must return exactly the same query results as
//! [`BruteForceQuery`] for the same object set (contract §14).

use grimoire_collide::BruteForceQuery;
use grimoire_collide::{
    Capsule, Circle, ColliderKey, CollisionQuery, GrazeRing, GridConfig, GridItem, LayerMask,
    MAX_COORD, Shape, SpatialGrid,
};
use grimoire_core::Vec2;
use proptest::prelude::*;

/// Coordinates in a moderate, well-conditioned range: items land inside, at the edge of, and well
/// outside the fixed test grid below without ever combining a magnitude near [`MAX_COORD`] with a
/// much smaller one in the same comparison.
///
/// `overlaps` computes exact results for `f32` inputs of *comparable* magnitude, but `f32` simply
/// cannot distinguish `1.0e9` from `1.0e9 + 0.25` (its precision near that magnitude is about
/// `64`) — that is a hard limit of the representation, not a bug this crate can compute around.
/// Combining a near-`MAX_COORD` shape with an unrelated small one (say, a bullet at the origin
/// against another bullet parked at `+MAX_COORD`) can then make the *exact* pairwise `overlaps`
/// call and the grid's exact-but-separately-rounded per-shape [`Aabb`] pruning round the same true
/// distance to different `f32` values and disagree right at the boundary. The contract's rationale
/// for `MAX_COORD` (§14) is to keep intermediates finite — avoiding `inf <= inf` false matches —
/// not to guarantee bit-identical outcomes across a nine-order-of-magnitude coordinate spread, so
/// this generator stays inside a well-conditioned range; `extreme_coordinates_in_opposite_edge_cells_agree`
/// below covers the actual `±MAX_COORD` scenario the contract calls out, with hand-picked values
/// where every intermediate rounds unambiguously.
fn coord() -> impl Strategy<Value = f32> {
    -1000.0f32..1000.0
}

/// Radii mostly small, occasionally `0`.
fn radius() -> impl Strategy<Value = f32> {
    prop_oneof![
        9 => 0.0f32..5.0,
        1 => Just(0.0),
    ]
}

fn vec2s() -> impl Strategy<Value = Vec2> {
    (coord(), coord()).prop_map(|(x, y)| Vec2::new(x, y))
}

/// Circles, ordinary capsules, and degenerate (`a == b`) capsules — filtered down to valid shapes
/// only, as the contract requires of this generator.
fn shape_strategy() -> impl Strategy<Value = Shape> {
    prop_oneof![
        (vec2s(), radius()).prop_map(|(center, radius)| Shape::Circle(Circle { center, radius })),
        (vec2s(), vec2s(), radius()).prop_map(|(a, b, radius)| Shape::Capsule(Capsule {
            a,
            b,
            radius
        })),
        (vec2s(), radius()).prop_map(|(a, radius)| Shape::Capsule(Capsule { a, b: a, radius })),
    ]
    .prop_filter("generator must only produce valid shapes", Shape::is_valid)
}

fn layer_mask_strategy() -> impl Strategy<Value = LayerMask> {
    prop::sample::select(vec![
        LayerMask::NONE,
        LayerMask::layer(0),
        LayerMask::layer(1),
        LayerMask::layer(0) | LayerMask::layer(1),
        LayerMask::ALL,
    ])
}

fn items_strategy(max_len: usize) -> impl Strategy<Value = Vec<GridItem>> {
    prop::collection::vec((shape_strategy(), layer_mask_strategy()), 0..=max_len).prop_map(|list| {
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

/// Coordinates tightly clustered around the origin, to stress many objects landing in the same
/// handful of grid cells.
fn cluster_coord() -> impl Strategy<Value = f32> {
    -0.5f32..0.5
}

fn cluster_items_strategy(max_len: usize) -> impl Strategy<Value = Vec<GridItem>> {
    prop::collection::vec(
        (
            (cluster_coord(), cluster_coord()),
            0.01f32..0.3,
            layer_mask_strategy(),
        ),
        1..=max_len,
    )
    .prop_map(|list| {
        list.into_iter()
            .enumerate()
            .map(|(index, ((x, y), radius, layers))| GridItem {
                key: ColliderKey::pool(index as u32, 0),
                shape: Shape::Circle(Circle {
                    center: Vec2::new(x, y),
                    radius,
                }),
                layers,
            })
            .collect()
    })
}

fn fixed_grid_config() -> GridConfig {
    GridConfig::new(Vec2::new(-20.0, -20.0), 2.0, 20, 20)
}

fn assert_same_results(items: &[GridItem], shape: &Shape, mask: LayerMask, ring: &GrazeRing) {
    let mut grid = SpatialGrid::new(fixed_grid_config()).expect("valid config");
    grid.rebuild(items.iter().copied());
    let brute = BruteForceQuery::new(items);

    let mut grid_hits = Vec::new();
    let mut brute_hits = Vec::new();
    grid.overlapping(shape, mask, &mut grid_hits);
    brute.overlapping(shape, mask, &mut brute_hits);
    assert_eq!(grid_hits, brute_hits, "overlapping diverged");

    let mut grid_ring = Vec::new();
    let mut brute_ring = Vec::new();
    grid.graze_ring(ring, mask, &mut grid_ring);
    brute.graze_ring(ring, mask, &mut brute_ring);
    assert_eq!(grid_ring, brute_ring, "graze_ring diverged");
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// General case: random valid shapes and objects, including degenerate capsules, radius 0,
    /// and objects outside the fixed grid's bounds (contract §14 conformance list).
    #[test]
    fn spatial_grid_matches_brute_force(
        items in items_strategy(80),
        probe_shape in shape_strategy(),
        probe_mask in layer_mask_strategy(),
        ring_center in vec2s(),
        inner_radius in 0.0f32..5.0,
        outer_extra in 0.0f32..5.0,
    ) {
        let ring = GrazeRing {
            center: ring_center,
            inner_radius,
            outer_radius: inner_radius + outer_extra,
        };
        assert_same_results(&items, &probe_shape, probe_mask, &ring);
    }

    /// Dense clusters: many small circles packed into a handful of cells.
    #[test]
    fn spatial_grid_matches_brute_force_in_dense_clusters(
        items in cluster_items_strategy(120),
        probe_shape in shape_strategy(),
        probe_mask in layer_mask_strategy(),
    ) {
        let ring = GrazeRing { center: Vec2::ZERO, inner_radius: 0.1, outer_radius: 0.4 };
        assert_same_results(&items, &probe_shape, probe_mask, &ring);
    }
}

/// Two shapes at `+MAX_COORD` / `-MAX_COORD` with radius `MAX_COORD` fall into opposite edge
/// cells of any bounded grid; `SpatialGrid` must still agree with `BruteForceQuery` on whether
/// they overlap (contract §14: this is exactly the scenario `MAX_COORD` is chosen to keep exact).
#[test]
fn extreme_coordinates_in_opposite_edge_cells_agree() {
    let far_apart = [
        GridItem {
            key: ColliderKey::pool(0, 0),
            shape: Shape::Circle(Circle {
                center: Vec2::new(-MAX_COORD, -MAX_COORD),
                radius: MAX_COORD,
            }),
            layers: LayerMask::ALL,
        },
        GridItem {
            key: ColliderKey::pool(1, 0),
            shape: Shape::Circle(Circle {
                center: Vec2::new(MAX_COORD, MAX_COORD),
                radius: MAX_COORD,
            }),
            layers: LayerMask::ALL,
        },
    ];
    let probe = Shape::Circle(Circle {
        center: Vec2::new(MAX_COORD, MAX_COORD),
        radius: 1.0,
    });
    let ring = GrazeRing {
        center: Vec2::ZERO,
        inner_radius: 0.0,
        outer_radius: 1.0,
    };
    assert_same_results(&far_apart, &probe, LayerMask::ALL, &ring);

    // Exactly touching along one axis at extreme scale: distance == sum of radii.
    let touching = [
        GridItem {
            key: ColliderKey::pool(0, 0),
            shape: Shape::Circle(Circle {
                center: Vec2::new(-MAX_COORD, 0.0),
                radius: MAX_COORD,
            }),
            layers: LayerMask::ALL,
        },
        GridItem {
            key: ColliderKey::pool(1, 0),
            shape: Shape::Circle(Circle {
                center: Vec2::new(MAX_COORD, 0.0),
                radius: MAX_COORD,
            }),
            layers: LayerMask::ALL,
        },
    ];
    let mut grid = SpatialGrid::new(fixed_grid_config()).expect("valid config");
    grid.rebuild(touching.iter().copied());
    let mut hits = Vec::new();
    grid.overlapping(&touching[0].shape, LayerMask::ALL, &mut hits);
    // Touching counts as overlap: item 0 overlaps itself and item 1.
    assert_eq!(hits.len(), 2);
    assert_same_results(&touching, &probe, LayerMask::ALL, &ring);
}

/// Exact touching (`distance == sum of radii`) must count as overlap through the grid too, not
/// only in the exact `overlaps` unit tests.
#[test]
fn exact_touching_counts_as_overlap_through_the_grid() {
    let items = [
        GridItem {
            key: ColliderKey::pool(0, 0),
            shape: Shape::Circle(Circle {
                center: Vec2::new(0.0, 0.0),
                radius: 1.0,
            }),
            layers: LayerMask::ALL,
        },
        GridItem {
            key: ColliderKey::pool(1, 0),
            shape: Shape::Circle(Circle {
                center: Vec2::new(2.0, 0.0),
                radius: 1.0,
            }),
            layers: LayerMask::ALL,
        },
    ];
    let mut grid = SpatialGrid::new(fixed_grid_config()).expect("valid config");
    grid.rebuild(items.iter().copied());
    let mut hits = Vec::new();
    grid.overlapping(
        &Shape::Circle(Circle {
            center: Vec2::new(0.0, 0.0),
            radius: 1.0,
        }),
        LayerMask::ALL,
        &mut hits,
    );
    assert_eq!(hits.len(), 2, "exact touching must count as overlap");
}
