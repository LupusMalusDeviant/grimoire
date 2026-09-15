//! `grimoire_collide` v0 (contract §14, §2a).

use std::ops::{BitAnd, BitAndAssign, BitOr, BitOrAssign, Not};

use grimoire_core::{StableHash, StableHasher, Vec2, impl_stable_hash};
use grimoire_ecs::{Entity, Executor};
use grimoire_ecs_p1::run_blocks;

#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct LayerMask(pub u32);

impl StableHash for LayerMask {
    fn stable_hash(&self, hasher: &mut StableHasher) {
        hasher.write_u32(self.0);
    }
}

impl LayerMask {
    pub const NONE: Self = Self(0);
    pub const ALL: Self = Self(u32::MAX);

    pub const fn layer(bit: u8) -> Self {
        assert!(bit < 32, "layer bit must be below 32");
        Self(1 << bit)
    }

    pub const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl BitOr for LayerMask {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}
impl BitAnd for LayerMask {
    type Output = Self;
    fn bitand(self, rhs: Self) -> Self {
        Self(self.0 & rhs.0)
    }
}
impl Not for LayerMask {
    type Output = Self;
    fn not(self) -> Self {
        Self(!self.0)
    }
}
impl BitOrAssign for LayerMask {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}
impl BitAndAssign for LayerMask {
    fn bitand_assign(&mut self, rhs: Self) {
        self.0 &= rhs.0;
    }
}

#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct Circle {
    pub center: Vec2,
    pub radius: f32,
}
impl_stable_hash!(Circle { center, radius });

#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct Capsule {
    pub a: Vec2,
    pub b: Vec2,
    pub radius: f32,
}
impl_stable_hash!(Capsule { a, b, radius });

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Shape {
    Circle(Circle),
    Capsule(Capsule),
}

impl StableHash for Shape {
    fn stable_hash(&self, hasher: &mut StableHasher) {
        match self {
            Shape::Circle(circle) => {
                hasher.write_u8(0);
                circle.stable_hash(hasher);
            }
            Shape::Capsule(capsule) => {
                hasher.write_u8(1);
                capsule.stable_hash(hasher);
            }
        }
    }
}

impl Shape {
    pub fn aabb(&self) -> Aabb {
        unimplemented!()
    }

    pub fn is_valid(&self) -> bool {
        unimplemented!()
    }
}

#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct Aabb {
    pub min: Vec2,
    pub max: Vec2,
}
impl_stable_hash!(Aabb { min, max });

pub fn overlaps(a: &Shape, b: &Shape) -> bool {
    let _ = (a, b);
    unimplemented!()
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ColliderKey {
    pub source: u8,
    pub index: u32,
    pub generation: u32,
}
impl_stable_hash!(ColliderKey {
    source,
    index,
    generation
});

impl ColliderKey {
    pub fn from_entity(entity: Entity) -> Self {
        Self {
            source: SOURCE_ENTITY,
            index: entity.index(),
            generation: entity.generation(),
        }
    }

    pub fn entity(self) -> Option<Entity> {
        (self.source == SOURCE_ENTITY)
            .then(|| Entity::from_bits((u64::from(self.generation) << 32) | u64::from(self.index)))
    }

    pub fn pool(index: u32, generation: u32) -> Self {
        Self {
            source: SOURCE_POOL,
            index,
            generation,
        }
    }
}

pub const SOURCE_ENTITY: u8 = 0;
pub const SOURCE_POOL: u8 = 1;

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Collider {
    pub shape: Shape,
    pub layers: LayerMask,
}
impl_stable_hash!(Collider { shape, layers });

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct GridItem {
    pub key: ColliderKey,
    pub shape: Shape,
    pub layers: LayerMask,
}
impl_stable_hash!(GridItem { key, shape, layers });

/// Contract: `Copy, Eq, PartialOrd, Ord (key, dann layers), Debug, StableHash` (derived in field order).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Hit {
    pub key: ColliderKey,
    pub layers: LayerMask,
}
impl_stable_hash!(Hit { key, layers });

#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct GrazeRing {
    pub center: Vec2,
    pub inner_radius: f32,
    pub outer_radius: f32,
}
impl_stable_hash!(GrazeRing {
    center,
    inner_radius,
    outer_radius
});

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ShapeQuery {
    pub shape: Shape,
    pub mask: LayerMask,
}

pub trait CollisionQuery: Send + Sync {
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn overlapping(&self, shape: &Shape, mask: LayerMask, out: &mut Vec<Hit>);
    fn graze_ring(&self, ring: &GrazeRing, mask: LayerMask, out: &mut Vec<Hit>);
}

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

#[derive(Debug)]
pub struct BruteForceQuery<'a> {
    items: &'a [GridItem],
}

impl<'a> BruteForceQuery<'a> {
    pub fn new(items: &'a [GridItem]) -> Self {
        Self { items }
    }
}

impl CollisionQuery for BruteForceQuery<'_> {
    fn len(&self) -> usize {
        self.items.len()
    }
    fn overlapping(&self, shape: &Shape, mask: LayerMask, out: &mut Vec<Hit>) {
        out.clear();
        out.extend(
            self.items
                .iter()
                .filter(|item| item.layers.intersects(mask) && overlaps(&item.shape, shape))
                .map(|item| Hit {
                    key: item.key,
                    layers: item.layers,
                }),
        );
        out.sort();
        out.dedup();
    }
    fn graze_ring(&self, ring: &GrazeRing, mask: LayerMask, out: &mut Vec<Hit>) {
        let _ = (ring, mask);
        out.clear();
    }
}

pub const MAX_GRID_CELLS: u64 = 1 << 20;

#[non_exhaustive]
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct GridConfig {
    pub origin: Vec2,
    pub cell_size: f32,
    pub columns: u32,
    pub rows: u32,
}
impl_stable_hash!(GridConfig {
    origin,
    cell_size,
    columns,
    rows
});

impl GridConfig {
    pub fn new(origin: Vec2, cell_size: f32, columns: u32, rows: u32) -> Self {
        Self {
            origin,
            cell_size,
            columns,
            rows,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SpatialGrid {
    config: GridConfig,
    items: Vec<GridItem>,
    cell_start: Vec<u32>,
    cell_items: Vec<u32>,
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
        let _ = (shape, mask, &self.cell_start, &self.cell_items);
        out.clear();
    }
    fn graze_ring(&self, ring: &GrazeRing, mask: LayerMask, out: &mut Vec<Hit>) {
        let _ = (ring, mask);
        out.clear();
    }
}

/// Cell range `[x0, x1] × [y0, y1]` of one item.
type CellRange = (u32, u32, u32, u32);

impl SpatialGrid {
    pub fn new(config: GridConfig) -> Result<Self, CollideError> {
        if !(config.cell_size.is_finite() && config.cell_size > 0.0) {
            return Err(CollideError::InvalidGridConfig("cell_size"));
        }
        Ok(Self {
            config,
            items: Vec::new(),
            cell_start: Vec::new(),
            cell_items: Vec::new(),
        })
    }

    #[must_use]
    pub fn config(&self) -> GridConfig {
        self.config
    }

    #[must_use]
    pub fn items(&self) -> &[GridItem] {
        &self.items
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.cell_start.clear();
        self.cell_items.clear();
    }

    pub fn rebuild(&mut self, items: impl IntoIterator<Item = GridItem>) {
        self.clear();
        self.items.extend(items);
    }

    fn cell_of(config: &GridConfig, p: Vec2) -> (u32, u32) {
        let cx = ((p.x - config.origin.x) / config.cell_size).floor() as i64;
        let cy = ((p.y - config.origin.y) / config.cell_size).floor() as i64;
        let clamp = |c: i64, n: u32| c.clamp(0, i64::from(n) - 1) as u32;
        (clamp(cx, config.columns), clamp(cy, config.rows))
    }

    pub fn rebuild_par(&mut self, executor: &dyn Executor, items: impl IntoIterator<Item = GridItem>) {
        self.clear();
        self.items.extend(items);
        let config = self.config;
        let blocks: Vec<&[GridItem]> = self.items.chunks(1024).collect();
        let ranges: Vec<Vec<CellRange>> = run_blocks(executor, blocks, |block: &[GridItem]| {
            block
                .iter()
                .map(|item| {
                    let aabb = item.shape.aabb();
                    let (x0, y0) = Self::cell_of(&config, aabb.min);
                    let (x1, y1) = Self::cell_of(&config, aabb.max);
                    (x0, x1, y0, y1)
                })
                .collect()
        });
        let _ = ranges;
    }

    pub fn overlapping_batch(&self, executor: &dyn Executor, queries: &[ShapeQuery], out: &mut BatchHits) {
        let blocks: Vec<&[ShapeQuery]> = queries.chunks(64).collect();
        let results: Vec<Vec<Vec<Hit>>> = run_blocks(executor, blocks, |block: &[ShapeQuery]| {
            block
                .iter()
                .map(|query| {
                    let mut hits = Vec::new();
                    self.overlapping(&query.shape, query.mask, &mut hits);
                    hits
                })
                .collect()
        });
        out.clear();
        for hits in results.into_iter().flatten() {
            out.hits.extend_from_slice(&hits);
            out.ends.push(out.hits.len());
        }
    }
}

#[derive(Clone, Default, Debug)]
pub struct BatchHits {
    ends: Vec<usize>,
    hits: Vec<Hit>,
}

impl BatchHits {
    pub fn len(&self) -> usize {
        self.ends.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ends.is_empty()
    }

    pub fn hits(&self, i: usize) -> &[Hit] {
        let start = if i == 0 { 0 } else { self.ends[i - 1] };
        &self.hits[start..self.ends[i]]
    }

    pub fn clear(&mut self) {
        self.ends.clear();
        self.hits.clear();
    }
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CollideError {
    #[error("invalid grid config: {0}")]
    InvalidGridConfig(&'static str),
}

pub const MAX_COORD: f32 = 1.0e9;

#[cfg(feature = "conformance")]
pub mod conformance {
    use super::{CollisionQuery, GridItem};

    pub fn collision_query<'a, Q: CollisionQuery>(
        items: &'a [GridItem],
        make: impl Fn(&'a [GridItem]) -> Q,
    ) {
        let _ = (items, make);
        unimplemented!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_resource<R: grimoire_ecs::Resource>() {}
    fn assert_component<C: grimoire_ecs::Component>() {}

    #[test]
    fn bounds() {
        assert_resource::<SpatialGrid>();
        assert_component::<Collider>();
        let _: Option<&dyn CollisionQuery> = None;
        assert_eq!(LayerMask::default(), LayerMask::NONE);
    }
}
