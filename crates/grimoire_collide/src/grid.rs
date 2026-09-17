//! Uniform-grid broadphase: [`GridConfig`], [`SpatialGrid`] and its batched query result type
//! [`BatchHits`] (contract §14).

use grimoire_core::{StableHash, StableHasher, Vec2, impl_stable_hash};
use grimoire_ecs::Executor;

use crate::collider::{GridItem, Hit};
use crate::layers::LayerMask;
use crate::query::{
    CollisionQuery, collect_graze_hits, collect_overlap_hits, graze_ring_is_valid, ring_shapes,
};
use crate::shapes::{Aabb, Shape};
use crate::{GrazeRing, ShapeQuery};

/// Upper bound on `columns * rows` for a [`GridConfig`] (contract §14).
pub const MAX_GRID_CELLS: u64 = 1 << 20;

/// Uniform grid geometry: cell size and extent, anchored at `origin`.
///
/// `#[non_exhaustive]` (contract §2 rule 13): future fields extend the geometry without breaking
/// existing callers. Validity — checked by [`SpatialGrid::new`], not here — requires `cell_size`
/// finite and `> 0`, `origin` finite, `columns >= 1`, `rows >= 1` and `columns * rows <=
/// `[`MAX_GRID_CELLS`]` (computed in `u64`).
#[non_exhaustive]
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct GridConfig {
    /// World position of the grid's `(0, 0)` cell corner.
    pub origin: Vec2,
    /// Side length of one (square) cell.
    pub cell_size: f32,
    /// Number of columns (cells along X).
    pub columns: u32,
    /// Number of rows (cells along Y).
    pub rows: u32,
}
impl_stable_hash!(GridConfig {
    origin,
    cell_size,
    columns,
    rows
});

impl GridConfig {
    /// Builds a configuration. Validity is checked by [`SpatialGrid::new`], not here, so a
    /// configuration can be constructed and inspected before deciding whether to build a grid.
    #[must_use]
    pub const fn new(origin: Vec2, cell_size: f32, columns: u32, rows: u32) -> Self {
        Self {
            origin,
            cell_size,
            columns,
            rows,
        }
    }
}

/// Errors constructing collision-crate state (contract §14).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CollideError {
    /// A [`GridConfig`] failed validation; the payload names the violated rule.
    #[error("invalid grid config: {0}")]
    InvalidGridConfig(&'static str),
}

/// Cell range `[x0, x1] x [y0, y1]` (inclusive) of one item's [`Aabb`], clamped to grid bounds.
type CellRange = (u32, u32, u32, u32);

/// Uniform-grid broadphase; also a [`grimoire_ecs::Resource`] — simulation state, part of
/// snapshots (contract §14).
///
/// [`StableHash`] feeds [`GridConfig`], then the object count, then each object in insertion
/// order (`key`, `shape`, `layers`); the derived cell structures are not hashed. After
/// [`grimoire_ecs::World::restore`](https://docs.rs/grimoire_ecs), queries return the same
/// results as at snapshot time.
#[derive(Clone, Debug)]
pub struct SpatialGrid {
    config: GridConfig,
    items: Vec<GridItem>,
    /// Start index of cell `c` in `cell_items` is `cell_start[c]`, end is `cell_start[c + 1]`.
    /// Length is always `columns * rows + 1`.
    cell_start: Vec<u32>,
    /// Item indices into `items`, grouped by cell, row-major (`y` outer, `x` inner).
    cell_items: Vec<u32>,
    /// Scratch buffer reused by [`Self::build_cells`] for the counting-sort insertion cursor.
    /// Purely an implementation detail: not public state, not hashed.
    cursor: Vec<u32>,
}

impl StableHash for SpatialGrid {
    fn stable_hash(&self, hasher: &mut StableHasher) {
        self.config.stable_hash(hasher);
        hasher.write_usize(self.items.len());
        for item in &self.items {
            item.stable_hash(hasher);
        }
    }
}

impl CollisionQuery for SpatialGrid {
    fn len(&self) -> usize {
        self.items.len()
    }

    fn overlapping(&self, shape: &Shape, mask: LayerMask, out: &mut Vec<Hit>) {
        let candidates = self.candidate_indices(shape.aabb());
        collect_overlap_hits(
            shape,
            mask,
            candidates.map(|index| &self.items[index as usize]),
            out,
        );
    }

    fn graze_ring(&self, ring: &GrazeRing, mask: LayerMask, out: &mut Vec<Hit>) {
        let valid = graze_ring_is_valid(ring);
        debug_assert!(valid, "invalid graze ring {ring:?}");
        out.clear();
        if !valid {
            return;
        }
        let (outer, _inner) = ring_shapes(ring);
        let candidates = self.candidate_indices(outer.aabb());
        collect_graze_hits(
            ring,
            mask,
            candidates.map(|index| &self.items[index as usize]),
            out,
        );
    }
}

impl SpatialGrid {
    /// Creates an empty grid, or [`CollideError::InvalidGridConfig`] if `config` fails validation
    /// (contract §14).
    pub fn new(config: GridConfig) -> Result<Self, CollideError> {
        if !config.cell_size.is_finite() || config.cell_size <= 0.0 {
            return Err(CollideError::InvalidGridConfig(
                "cell_size must be finite and > 0",
            ));
        }
        if !config.origin.x.is_finite() || !config.origin.y.is_finite() {
            return Err(CollideError::InvalidGridConfig("origin must be finite"));
        }
        if config.columns == 0 {
            return Err(CollideError::InvalidGridConfig("columns must be >= 1"));
        }
        if config.rows == 0 {
            return Err(CollideError::InvalidGridConfig("rows must be >= 1"));
        }
        if u64::from(config.columns) * u64::from(config.rows) > MAX_GRID_CELLS {
            return Err(CollideError::InvalidGridConfig(
                "columns * rows must not exceed MAX_GRID_CELLS",
            ));
        }
        let cell_count = config.columns as usize * config.rows as usize;
        Ok(Self {
            config,
            items: Vec::new(),
            cell_start: vec![0; cell_count + 1],
            cell_items: Vec::new(),
            cursor: vec![0; cell_count + 1],
        })
    }

    /// The grid's geometry.
    #[must_use]
    pub fn config(&self) -> GridConfig {
        self.config
    }

    /// Entered objects, in insertion order.
    #[must_use]
    pub fn items(&self) -> &[GridItem] {
        &self.items
    }

    /// Empties the grid, keeping its allocations.
    pub fn clear(&mut self) {
        self.items.clear();
        self.cell_items.clear();
        self.cell_start.fill(0);
    }

    /// Replaces the grid's content with `items` (contract §14: an implicit [`Self::clear`] runs
    /// first).
    ///
    /// # Panics
    ///
    /// In debug builds, panics if any item's shape is invalid ([`Shape::is_valid`]) or two items
    /// share a [`crate::ColliderKey`]. Release builds stay panic-free with functionally
    /// unspecified but deterministic behaviour.
    pub fn rebuild(&mut self, items: impl IntoIterator<Item = GridItem>) {
        self.clear();
        self.items.extend(items);
        #[cfg(debug_assertions)]
        debug_validate(&self.items);
        let config = self.config;
        let ranges: Vec<CellRange> = self
            .items
            .iter()
            .map(|item| cell_range_of(&config, item.shape.aabb()))
            .collect();
        self.build_cells(&ranges);
    }

    /// Data-parallel form of [`Self::rebuild`] (contract §14, engine ADR-0006): reads `items`
    /// sequentially into a buffer, then computes each object's cell range in fixed blocks through
    /// `executor` (a pure mapping, no reduction); the resulting grid — content, iteration and
    /// hash — is byte-identical to [`Self::rebuild`] for any executor.
    ///
    /// # Panics
    ///
    /// Same shape/key panics as [`Self::rebuild`] in debug builds. If a block panics, the panic
    /// with the lowest block index is resumed once every block has finished, and the grid is left
    /// empty.
    pub fn rebuild_par(
        &mut self,
        executor: &dyn Executor,
        items: impl IntoIterator<Item = GridItem>,
    ) {
        self.clear();
        let items: Vec<GridItem> = items.into_iter().collect();
        #[cfg(debug_assertions)]
        debug_validate(&items);
        let config = self.config;
        let blocks: Vec<&[GridItem]> = grimoire_ecs::slice_block_ranges(items.len())
            .map(|range| &items[range])
            .collect();
        let ranges: Vec<CellRange> =
            grimoire_ecs::run_blocks(executor, blocks, |block: &[GridItem]| {
                block
                    .iter()
                    .map(|item| cell_range_of(&config, item.shape.aabb()))
                    .collect::<Vec<_>>()
            })
            .into_iter()
            .flatten()
            .collect();
        // Reached only if no block panicked (`run_blocks` resumes the lowest-index panic before
        // returning); until here `self` stays cleared, so a panic leaves the grid empty.
        self.items = items;
        self.build_cells(&ranges);
    }

    /// Data-parallel form of [`CollisionQuery::overlapping`] over many queries at once (contract
    /// §14): splits `queries` into fixed blocks through `executor`; `out.hits(i)` is
    /// bit-identical to calling [`CollisionQuery::overlapping`] with `queries[i]`, assembled in
    /// query order.
    ///
    /// Read-only: a block panic leaves the grid unchanged and `out` cleared.
    pub fn overlapping_batch(
        &self,
        executor: &dyn Executor,
        queries: &[ShapeQuery],
        out: &mut BatchHits,
    ) {
        out.clear();
        let blocks: Vec<&[ShapeQuery]> = grimoire_ecs::slice_block_ranges(queries.len())
            .map(|range| &queries[range])
            .collect();
        let batches: Vec<Vec<Vec<Hit>>> =
            grimoire_ecs::run_blocks(executor, blocks, |block: &[ShapeQuery]| {
                block
                    .iter()
                    .map(|query| {
                        let mut hits = Vec::new();
                        self.overlapping(&query.shape, query.mask, &mut hits);
                        hits
                    })
                    .collect::<Vec<_>>()
            });
        // Reached only if no block panicked; `out` was already cleared above.
        for hits in batches.into_iter().flatten() {
            out.push(&hits);
        }
    }

    /// Item indices (with duplicates across cells) whose cell overlaps `aabb`'s candidate cell
    /// range.
    fn candidate_indices(&self, aabb: Aabb) -> impl Iterator<Item = u32> + '_ {
        let (x0, x1, y0, y1) = cell_range_of(&self.config, aabb);
        (y0..=y1).flat_map(move |cy| {
            (x0..=x1).flat_map(move |cx| {
                let cell = cell_index(&self.config, cx, cy);
                let start = self.cell_start[cell] as usize;
                let end = self.cell_start[cell + 1] as usize;
                self.cell_items[start..end].iter().copied()
            })
        })
    }

    /// Counting-sort insertion of every item into `cell_start`/`cell_items`, row-major (`y`
    /// outer, `x` inner; contract §14). `ranges[i]` is the cell span of `self.items[i]`.
    fn build_cells(&mut self, ranges: &[CellRange]) {
        let cell_count = self.config.columns as usize * self.config.rows as usize;
        self.cell_start.fill(0);
        for &(x0, x1, y0, y1) in ranges {
            for cy in y0..=y1 {
                for cx in x0..=x1 {
                    self.cell_start[cell_index(&self.config, cx, cy) + 1] += 1;
                }
            }
        }
        for i in 0..cell_count {
            self.cell_start[i + 1] += self.cell_start[i];
        }
        let total = self.cell_start[cell_count] as usize;
        self.cell_items.clear();
        self.cell_items.resize(total, 0);
        self.cursor.clear();
        self.cursor.extend_from_slice(&self.cell_start);
        for (item_index, &(x0, x1, y0, y1)) in ranges.iter().enumerate() {
            for cy in y0..=y1 {
                for cx in x0..=x1 {
                    let cell = cell_index(&self.config, cx, cy);
                    let slot = &mut self.cursor[cell];
                    self.cell_items[*slot as usize] = item_index as u32;
                    *slot += 1;
                }
            }
        }
    }
}

/// In debug builds, panics if any item's shape is invalid or two items share a key (contract
/// §14). Compiled away entirely in release builds.
#[cfg(debug_assertions)]
fn debug_validate(items: &[GridItem]) {
    use std::collections::BTreeSet;

    let mut seen = BTreeSet::new();
    for item in items {
        debug_assert!(
            item.shape.is_valid(),
            "invalid shape for collider {:?}",
            item.key
        );
        debug_assert!(
            seen.insert(item.key),
            "duplicate collider key {:?}",
            item.key
        );
    }
}

/// Cell coordinates of a point: `floor((p - origin) / cell_size)`, saturating on overflow (a plain
/// `as` cast from float to `i64` is already saturating), then clamped to the grid's bounds.
/// Out-of-bounds points land in edge cells (contract §14).
fn cell_of(config: &GridConfig, p: Vec2) -> (u32, u32) {
    let cx = ((p.x - config.origin.x) / config.cell_size).floor() as i64;
    let cy = ((p.y - config.origin.y) / config.cell_size).floor() as i64;
    (
        clamp_to_axis(cx, config.columns),
        clamp_to_axis(cy, config.rows),
    )
}

fn clamp_to_axis(c: i64, len: u32) -> u32 {
    c.clamp(0, i64::from(len) - 1) as u32
}

fn cell_range_of(config: &GridConfig, aabb: Aabb) -> CellRange {
    let (x0, y0) = cell_of(config, aabb.min);
    let (x1, y1) = cell_of(config, aabb.max);
    (x0, x1, y0, y1)
}

fn cell_index(config: &GridConfig, cx: u32, cy: u32) -> usize {
    cy as usize * config.columns as usize + cx as usize
}

/// Results of [`SpatialGrid::overlapping_batch`]: one hit list per query, in query order.
#[derive(Clone, Default, Debug)]
pub struct BatchHits {
    ends: Vec<usize>,
    hits: Vec<Hit>,
}

impl BatchHits {
    /// Number of queries this batch holds results for.
    #[must_use]
    pub fn len(&self) -> usize {
        self.ends.len()
    }

    /// Whether the batch holds no query results.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ends.is_empty()
    }

    /// Hits of query `i`, ascending by [`crate::ColliderKey`] with no duplicates.
    ///
    /// # Panics
    ///
    /// Panics if `i >= self.len()`.
    #[must_use]
    pub fn hits(&self, i: usize) -> &[Hit] {
        let start = if i == 0 { 0 } else { self.ends[i - 1] };
        &self.hits[start..self.ends[i]]
    }

    /// Empties the batch, keeping its allocations.
    pub fn clear(&mut self) {
        self.ends.clear();
        self.hits.clear();
    }

    fn push(&mut self, hits: &[Hit]) {
        self.hits.extend_from_slice(hits);
        self.ends.push(self.hits.len());
    }
}

#[cfg(test)]
mod tests {
    use grimoire_core::hash_of;
    use grimoire_ecs::SequentialExecutor;

    use super::*;
    use crate::key::ColliderKey;
    use crate::shapes::Circle;

    fn small_config() -> GridConfig {
        GridConfig::new(Vec2::new(0.0, 0.0), 1.0, 4, 4)
    }

    fn item(index: u32, x: f32, y: f32) -> GridItem {
        GridItem {
            key: ColliderKey::pool(index, 0),
            shape: Shape::Circle(Circle {
                center: Vec2::new(x, y),
                radius: 0.4,
            }),
            layers: LayerMask::ALL,
        }
    }

    #[test]
    fn grid_config_validity_boundaries() {
        assert!(SpatialGrid::new(GridConfig::new(Vec2::ZERO, 0.0, 1, 1)).is_err());
        assert!(SpatialGrid::new(GridConfig::new(Vec2::ZERO, -1.0, 1, 1)).is_err());
        assert!(SpatialGrid::new(GridConfig::new(Vec2::new(f32::NAN, 0.0), 1.0, 1, 1)).is_err());
        assert!(SpatialGrid::new(GridConfig::new(Vec2::ZERO, 1.0, 0, 1)).is_err());
        assert!(SpatialGrid::new(GridConfig::new(Vec2::ZERO, 1.0, 1, 0)).is_err());
        // columns * rows == MAX_GRID_CELLS is accepted...
        assert!(SpatialGrid::new(GridConfig::new(Vec2::ZERO, 1.0, 1 << 10, 1 << 10)).is_ok());
        // ... one more cell is rejected.
        assert!(
            SpatialGrid::new(GridConfig::new(Vec2::ZERO, 1.0, (1 << 10) + 1, 1 << 10)).is_err()
        );
    }

    #[test]
    fn clear_empties_the_grid_and_a_query_afterwards_finds_nothing() {
        let mut grid = SpatialGrid::new(small_config()).unwrap();
        grid.rebuild([item(0, 0.5, 0.5), item(1, 1.5, 1.5)]);
        assert_eq!(grid.items().len(), 2);
        grid.clear();
        assert!(grid.items().is_empty());
        assert_eq!(grid.len(), 0);
        assert!(grid.is_empty());
        let mut out = Vec::new();
        grid.overlapping(
            &Shape::Circle(Circle {
                center: Vec2::new(0.5, 0.5),
                radius: 0.5,
            }),
            LayerMask::ALL,
            &mut out,
        );
        assert!(out.is_empty());
    }

    #[test]
    fn rebuild_replaces_content_fully() {
        let mut grid = SpatialGrid::new(small_config()).unwrap();
        grid.rebuild([item(0, 0.5, 0.5)]);
        assert_eq!(grid.items().len(), 1);
        grid.rebuild([item(1, 1.5, 1.5), item(2, 2.5, 2.5)]);
        assert_eq!(grid.items().len(), 2);
        assert_eq!(grid.items()[0].key, ColliderKey::pool(1, 0));
    }

    #[test]
    fn overlapping_matches_expected_hits_and_is_sorted() {
        let mut grid = SpatialGrid::new(small_config()).unwrap();
        grid.rebuild([item(0, 0.5, 0.5), item(1, 3.5, 3.5), item(2, 0.6, 0.5)]);
        let mut out = Vec::new();
        grid.overlapping(
            &Shape::Circle(Circle {
                center: Vec2::new(0.5, 0.5),
                radius: 0.5,
            }),
            LayerMask::ALL,
            &mut out,
        );
        assert_eq!(
            out,
            vec![
                Hit {
                    key: ColliderKey::pool(0, 0),
                    layers: LayerMask::ALL
                },
                Hit {
                    key: ColliderKey::pool(2, 0),
                    layers: LayerMask::ALL
                },
            ]
        );
    }

    #[test]
    fn points_outside_grid_bounds_land_in_edge_cells_and_are_still_found() {
        let mut grid = SpatialGrid::new(small_config()).unwrap();
        grid.rebuild([item(0, -100.0, -100.0), item(1, 100.0, 100.0)]);
        let mut out = Vec::new();
        grid.overlapping(
            &Shape::Circle(Circle {
                center: Vec2::new(-100.0, -100.0),
                radius: 0.5,
            }),
            LayerMask::ALL,
            &mut out,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].key, ColliderKey::pool(0, 0));
    }

    #[test]
    fn hash_matches_between_rebuild_and_rebuild_par() {
        let mut sequential = SpatialGrid::new(small_config()).unwrap();
        sequential.rebuild([item(0, 0.5, 0.5), item(1, 2.5, 2.5)]);
        let mut parallel = SpatialGrid::new(small_config()).unwrap();
        parallel.rebuild_par(&SequentialExecutor, [item(0, 0.5, 0.5), item(1, 2.5, 2.5)]);
        assert_eq!(hash_of(&sequential), hash_of(&parallel));
    }

    #[test]
    fn overlapping_batch_matches_sequential_overlapping() {
        let mut grid = SpatialGrid::new(small_config()).unwrap();
        grid.rebuild([item(0, 0.5, 0.5), item(1, 1.5, 1.5)]);
        let queries = [
            ShapeQuery {
                shape: Shape::Circle(Circle {
                    center: Vec2::new(0.5, 0.5),
                    radius: 0.5,
                }),
                mask: LayerMask::ALL,
            },
            ShapeQuery {
                shape: Shape::Circle(Circle {
                    center: Vec2::new(10.0, 10.0),
                    radius: 0.5,
                }),
                mask: LayerMask::ALL,
            },
        ];
        let mut batch = BatchHits::default();
        grid.overlapping_batch(&SequentialExecutor, &queries, &mut batch);
        assert_eq!(batch.len(), 2);
        let mut expected = Vec::new();
        grid.overlapping(&queries[0].shape, queries[0].mask, &mut expected);
        assert_eq!(batch.hits(0), expected.as_slice());
        assert!(batch.hits(1).is_empty());
    }

    #[test]
    #[cfg_attr(
        not(debug_assertions),
        ignore = "the shape validation is a debug assertion; release builds do not panic here"
    )]
    #[should_panic(expected = "invalid shape for collider")]
    fn rebuild_panics_in_debug_on_invalid_shape() {
        let mut grid = SpatialGrid::new(small_config()).unwrap();
        grid.rebuild([GridItem {
            key: ColliderKey::pool(0, 0),
            shape: Shape::Circle(Circle {
                center: Vec2::ZERO,
                radius: -1.0,
            }),
            layers: LayerMask::ALL,
        }]);
    }

    #[test]
    #[cfg_attr(
        not(debug_assertions),
        ignore = "the duplicate-key check is a debug assertion; release builds do not panic here"
    )]
    #[should_panic(expected = "duplicate collider key")]
    fn rebuild_panics_in_debug_on_duplicate_key() {
        let mut grid = SpatialGrid::new(small_config()).unwrap();
        grid.rebuild([item(0, 0.1, 0.1), item(0, 0.2, 0.2)]);
    }

    #[cfg(all(test, feature = "conformance"))]
    mod conformance_tests {
        use super::*;

        fn sample_grid() -> SpatialGrid {
            let mut grid =
                SpatialGrid::new(GridConfig::new(Vec2::new(-10.0, -10.0), 1.0, 20, 20)).unwrap();
            grid.rebuild([
                item(0, 0.0, 0.0),
                item(1, 0.5, 0.0),
                GridItem {
                    key: ColliderKey::pool(2, 0),
                    shape: Shape::Circle(Circle {
                        center: Vec2::new(-3.0, -3.0),
                        radius: 0.2,
                    }),
                    layers: LayerMask::NONE,
                },
            ]);
            grid
        }

        #[test]
        fn spatial_grid_is_conformant() {
            let grid = sample_grid();
            let shape = Shape::Circle(Circle {
                center: Vec2::ZERO,
                radius: 1.5,
            });
            let ring = GrazeRing {
                center: Vec2::ZERO,
                inner_radius: 0.2,
                outer_radius: 3.0,
            };
            crate::conformance::collision_query(&grid, &shape, &ring);
        }
    }
}
