//! `BulletPool`: the SoA bullet store (contract §11.3). Built as a self-contained data structure
//! in WP1.3; WP5.1 adds the mutable, data-parallel block view ([`PoolUpdateBlock`],
//! [`BulletPool::update_blocks_mut`]) and the small bookkeeping hooks (`begin_tick`,
//! `record_dropped_spawn`, `set_bounds`, `despawn_pending`) the `sigil.*` tick-phase systems
//! (`crate::systems`) need, without touching the hash layout or the public spawn/despawn API.

use std::collections::VecDeque;
use std::fmt;
use std::ops::Range;

use grimoire_core::{StableHash, StableHasher, Vec2, impl_stable_hash};
use grimoire_ecs::{QUERY_BLOCK_SIZE, slice_block_ranges};

use crate::content::{BulletFlags, SigilContent};
use crate::emitter::ClearFilter;
use crate::error::SigilError;
use crate::unit::{SigilUnit, UnitId};

/// Stable (slot index, generation) handle to a bullet.
///
/// Ordering compares the index first and the generation second (contract §11.3). After a slot is
/// reused, an old handle to it is no longer alive: [`BulletPool::is_alive`] returns `false` and
/// [`BulletPool::despawn`] returns `false`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BulletId {
    index: u32,
    generation: u32,
}

impl BulletId {
    /// Slot index inside the pool.
    #[must_use]
    pub const fn index(self) -> u32 {
        self.index
    }

    /// Generation of the slot at the time this handle was issued.
    #[must_use]
    pub const fn generation(self) -> u32 {
        self.generation
    }

    /// Packs the handle into 64 bits: index in the upper half, generation in the lower half.
    #[must_use]
    pub const fn to_bits(self) -> u64 {
        ((self.index as u64) << 32) | self.generation as u64
    }

    /// Inverse of [`BulletId::to_bits`].
    #[must_use]
    pub const fn from_bits(bits: u64) -> Self {
        Self {
            index: (bits >> 32) as u32,
            generation: bits as u32,
        }
    }
}

impl fmt::Display for BulletId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}v{}", self.index, self.generation)
    }
}

impl StableHash for BulletId {
    fn stable_hash(&self, hasher: &mut StableHasher) {
        hasher.write_u32(self.index);
        hasher.write_u32(self.generation);
    }
}

/// Request to spawn one bullet, passed to [`BulletPool::spawn`].
///
/// `#[non_exhaustive]`: constructed through [`BulletSpawn::new`] plus the `with_*` builders, never
/// through struct-literal syntax, so new optional fields can be added later.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct BulletSpawn {
    /// Unit the bullet type and program come from.
    pub unit: UnitId,
    /// Bullet-type index within the unit.
    pub bullet_type: u16,
    /// Program index within the unit; `0` means no modifiers.
    pub program: u16,
    /// Spawn position.
    pub position: Vec2,
    /// Spawn heading, in radians.
    pub angle: f32,
    /// Spawn speed.
    pub speed: f32,
    /// Sub-spawn cascade depth; `0` for a directly emitted bullet.
    pub cascade: u8,
}

impl BulletSpawn {
    /// Builds a request with `program: 0` and `cascade: 0`.
    #[must_use]
    pub const fn new(
        unit: UnitId,
        bullet_type: u16,
        position: Vec2,
        angle: f32,
        speed: f32,
    ) -> Self {
        Self {
            unit,
            bullet_type,
            program: 0,
            position,
            angle,
            speed,
            cascade: 0,
        }
    }

    /// Sets the program index.
    #[must_use]
    pub const fn with_program(mut self, program: u16) -> Self {
        self.program = program;
        self
    }

    /// Sets the cascade depth.
    #[must_use]
    pub const fn with_cascade(mut self, cascade: u8) -> Self {
        self.cascade = cascade;
        self
    }
}

/// Why a bullet was despawned.
///
/// `#[non_exhaustive]`: `Lifetime` and `Bounds` are produced by `sigil.update`/`sigil.resolve`
/// since WP5.1, `Transform` (`burst`, `become_emitter`) and `Behavior` since WP5.2. `Swap` stays
/// unproduced until hot-swap (§11.8, WP5.5); it is listed regardless because [`BulletEvent`] and
/// the golden pool hash already need a stable, complete set of tags.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DespawnCause {
    /// `lifetime_ticks` elapsed.
    Lifetime,
    /// The bullet left the simulation bounds.
    Bounds,
    /// A pattern transform removed the bullet.
    Transform,
    /// A [`crate::BehaviorOutcome::Despawn`] result.
    Behavior,
    /// [`BulletPool::clear`] or an applied [`crate::ClearRequest`].
    Clear,
    /// A content hot-swap invalidated the bullet's unit.
    Swap,
    /// Any other caller-driven despawn.
    External,
}

impl StableHash for DespawnCause {
    fn stable_hash(&self, hasher: &mut StableHasher) {
        hasher.write_u8(*self as u8);
    }
}

/// One despawn record, emitted by [`BulletPool::despawn`] and [`BulletPool::clear`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BulletEvent {
    /// Handle of the despawned bullet.
    pub id: BulletId,
    /// Unit the bullet was spawned from.
    pub unit: UnitId,
    /// Bullet-type index the bullet had.
    pub bullet_type: u16,
    /// Position at the time of despawn.
    pub position: Vec2,
    /// Why the bullet was despawned.
    pub cause: DespawnCause,
}
impl_stable_hash!(BulletEvent {
    id,
    unit,
    bullet_type,
    position,
    cause
});

/// One despawn decided by a `sigil.update` block (contract §11.6): the parallel block that found
/// slot `index` beyond its `lifetime_ticks` or outside the simulation bounds cannot touch the
/// pool's shared free list itself (it only owns a disjoint sub-slice of the SoA columns), so it
/// returns this instead. `sigil.resolve` applies every block's list, in block order, through
/// [`BulletPool::despawn_pending`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PendingDespawn {
    pub(crate) index: u32,
    pub(crate) cause: DespawnCause,
}

/// One sub-spawn decided by a `sigil.update` block (a `burst` or `become_emitter` transform,
/// contract §11.6), applied by `sigil.resolve` after every despawn of the tick. `parent_cascade`
/// is the transforming bullet's own depth; `sigil.resolve` adds the one level with `checked_add`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PendingSpawn {
    pub(crate) unit_index: u16,
    pub(crate) bullet_type: u16,
    /// Pool program numbering (one-based, `0` = none; see [`BulletPool::spawn`]).
    pub(crate) program: u16,
    pub(crate) position: Vec2,
    pub(crate) angle: f32,
    pub(crate) speed: f32,
    pub(crate) parent_cascade: u8,
}

/// Read-only view of one live or dead bullet slot, only ever constructed by [`BulletPool`].
#[derive(Debug, Clone, Copy)]
pub struct BulletRef<'p> {
    pool: &'p BulletPool,
    index: usize,
}

impl<'p> BulletRef<'p> {
    /// Handle of this bullet.
    #[must_use]
    pub fn id(&self) -> BulletId {
        BulletId {
            index: self.index as u32,
            generation: self.pool.generation[self.index],
        }
    }

    /// Index of the bullet's unit inside the loaded library (see
    /// [`crate::SigilLibrary::unit_index`]).
    #[must_use]
    pub fn unit_index(&self) -> u16 {
        self.pool.unit[self.index]
    }

    /// Bullet-type index within the unit.
    #[must_use]
    pub fn bullet_type(&self) -> u16 {
        self.pool.bullet_type[self.index]
    }

    /// Behaviour flags of the bullet's type.
    #[must_use]
    pub fn flags(&self) -> BulletFlags {
        self.pool.flags[self.index]
    }

    /// Current position.
    #[must_use]
    pub fn position(&self) -> Vec2 {
        self.pool.position[self.index]
    }

    /// Position at the end of the previous tick.
    #[must_use]
    pub fn previous_position(&self) -> Vec2 {
        self.pool.previous_position[self.index]
    }

    /// Current velocity.
    #[must_use]
    pub fn velocity(&self) -> Vec2 {
        self.pool.velocity[self.index]
    }

    /// Ticks since the bullet was spawned.
    #[must_use]
    pub fn age(&self) -> u32 {
        self.pool.age[self.index]
    }
}

/// Structure-of-arrays view over the slots `0..slot_count` of a [`BulletPool`] (or a sub-range, for
/// [`PoolBlock::columns`]).
///
/// `#[non_exhaustive]` and privately constructed (contract §2 rule 13): only [`BulletPool::columns`]
/// and [`PoolBlock::columns`] build one, so a future field can be added without breaking callers.
/// Field order here is also the order [`BulletPool`]'s stable hash feeds live-slot columns in.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct BulletColumns<'p> {
    /// Whether each slot currently holds a live bullet.
    pub alive: &'p [bool],
    /// Generation of each slot.
    pub generation: &'p [u32],
    /// Library unit index of each slot's bullet.
    pub unit: &'p [u16],
    /// Bullet-type index of each slot's bullet.
    pub bullet_type: &'p [u16],
    /// Program index of each slot's bullet.
    pub program: &'p [u16],
    /// Behaviour flags of each slot's bullet.
    pub flags: &'p [BulletFlags],
    /// Sub-spawn cascade depth of each slot's bullet.
    pub cascade: &'p [u8],
    /// Current position of each slot's bullet.
    pub position: &'p [Vec2],
    /// Previous-tick position of each slot's bullet.
    pub previous_position: &'p [Vec2],
    /// Current velocity of each slot's bullet.
    pub velocity: &'p [Vec2],
    /// Current heading of each slot's bullet, in radians.
    pub angle: &'p [f32],
    /// Current speed of each slot's bullet.
    pub speed: &'p [f32],
    /// Age in ticks of each slot's bullet.
    pub age: &'p [u32],
    /// Free-form behaviour state of each slot's bullet.
    pub state: &'p [[f32; 4]],
}

/// One data-parallel block of pool slots, as produced by [`BulletPool::blocks`].
#[derive(Debug, Clone)]
pub struct PoolBlock<'p> {
    index: usize,
    slots: Range<usize>,
    columns: BulletColumns<'p>,
}

impl<'p> PoolBlock<'p> {
    /// Block index (`slots().start / grimoire_ecs::QUERY_BLOCK_SIZE`).
    #[must_use]
    pub const fn index(&self) -> usize {
        self.index
    }

    /// Slot range covered by this block.
    #[must_use]
    pub fn slots(&self) -> Range<usize> {
        self.slots.clone()
    }

    /// Column view restricted to this block's slot range.
    #[must_use]
    pub const fn columns(&self) -> BulletColumns<'p> {
        self.columns
    }
}

/// One data-parallel, *mutable* block of pool slots for `sigil.update` (contract §11.7), as
/// produced by [`BulletPool::update_blocks_mut`].
///
/// Every column is a disjoint sub-slice of the pool's own `Vec`s, split with `split_at_mut` (no
/// `unsafe`, matching the contract's "ohne unsafe" requirement), so blocks can run through
/// [`grimoire_ecs::run_blocks`] without aliasing. The generation column is intentionally omitted;
/// `unit`, `program` and `cascade` are read-only because no transform changes them in place, while
/// `bullet_type` and `flags` are writable for `change_type` (WP5.2).
pub(crate) struct PoolUpdateBlock<'p> {
    index: usize,
    slots: Range<usize>,
    pub(crate) alive: &'p [bool],
    pub(crate) unit: &'p [u16],
    pub(crate) bullet_type: &'p mut [u16],
    pub(crate) program: &'p [u16],
    pub(crate) flags: &'p mut [BulletFlags],
    pub(crate) cascade: &'p [u8],
    pub(crate) position: &'p mut [Vec2],
    pub(crate) previous_position: &'p mut [Vec2],
    pub(crate) velocity: &'p mut [Vec2],
    pub(crate) angle: &'p mut [f32],
    pub(crate) speed: &'p mut [f32],
    pub(crate) age: &'p mut [u32],
    pub(crate) state: &'p mut [[f32; 4]],
}

impl PoolUpdateBlock<'_> {
    /// Block index (`slots().start / grimoire_ecs::QUERY_BLOCK_SIZE`); the key of the block's
    /// `derive_block_rng` stream (contract §11.7), drawn from by behaviors and by `scatter` blocks
    /// of sub-spawns.
    pub(crate) const fn index(&self) -> usize {
        self.index
    }

    /// Slot range covered by this block.
    pub(crate) fn slots(&self) -> Range<usize> {
        self.slots.clone()
    }
}

/// Splits `slice` into consecutive immutable sub-slices of at most [`QUERY_BLOCK_SIZE`] elements
/// each, matching [`slice_block_ranges`] exactly.
fn split_ref_blocks<T>(slice: &[T]) -> Vec<&[T]> {
    let mut rest = slice;
    let mut out = Vec::new();
    while !rest.is_empty() {
        let take = QUERY_BLOCK_SIZE.min(rest.len());
        let (head, tail) = rest.split_at(take);
        out.push(head);
        rest = tail;
    }
    out
}

/// Mutable counterpart of [`split_ref_blocks`], built with `split_at_mut` only.
fn split_mut_blocks<T>(slice: &mut [T]) -> Vec<&mut [T]> {
    let mut rest = slice;
    let mut out = Vec::new();
    while !rest.is_empty() {
        let take = QUERY_BLOCK_SIZE.min(rest.len());
        let (head, tail) = rest.split_at_mut(take);
        out.push(head);
        rest = tail;
    }
    out
}

/// Structure-of-arrays store of bullets with a stable-handle, generational free list, exactly like
/// the ECS entity allocator (contract §7/§11.3).
///
/// Every column is fully allocated to `capacity` slots by [`BulletPool::with_capacity`];
/// `slot_count` is the high-water mark of ever-used slots, and dead-slot contents are unspecified
/// (never read outside this type). `BulletPool::default()` has capacity `0` and performs no
/// allocation.
#[derive(Debug, Clone, Default)]
pub struct BulletPool {
    capacity: u32,
    slot_count: u32,
    alive_count: u32,
    /// Simulation bounds (contract §11.3, §11.6 `SigilConfig`), checked by `sigil.update`'s bounds
    /// step (`crate::runtime`) and written once by `install` (`BulletPool::set_bounds`). Stay at
    /// their `Default` (`Vec2::ZERO`) for a pool built directly (e.g. this crate's own unit tests)
    /// rather than through `install`.
    bounds_min: Vec2,
    bounds_max: Vec2,
    dropped_spawns: u64,
    free_list: VecDeque<u32>,
    alive: Vec<bool>,
    generation: Vec<u32>,
    unit: Vec<u16>,
    /// Full unit id per slot, kept only so [`BulletPool::despawn`] — which the contract does not
    /// pass a [`SigilContent`] to — can still populate [`BulletEvent::unit`] without a content
    /// lookup. Not part of [`BulletColumns`] (which stores the `u16` library index, matching the
    /// contract exactly) and not fed into [`BulletPool`]'s stable hash. Documented as a
    /// contract-change candidate in the WP1.3 report.
    unit_id: Vec<UnitId>,
    bullet_type: Vec<u16>,
    program: Vec<u16>,
    flags: Vec<BulletFlags>,
    cascade: Vec<u8>,
    position: Vec<Vec2>,
    previous_position: Vec<Vec2>,
    velocity: Vec<Vec2>,
    angle: Vec<f32>,
    speed: Vec<f32>,
    age: Vec<u32>,
    state: Vec<[f32; 4]>,
    events: Vec<BulletEvent>,
    events_tick: u64,
}

impl BulletPool {
    /// Largest allowed capacity.
    pub const MAX_CAPACITY: u32 = 1 << 20;

    /// Builds a pool with every column fully allocated to `capacity` slots.
    ///
    /// # Panics
    ///
    /// Panics if `capacity` exceeds [`BulletPool::MAX_CAPACITY`]; this is an invalid construction
    /// argument, not runtime data (contract §2 rule 6).
    #[must_use]
    pub fn with_capacity(capacity: u32) -> Self {
        assert!(
            capacity <= Self::MAX_CAPACITY,
            "BulletPool::with_capacity: {capacity} exceeds MAX_CAPACITY ({})",
            Self::MAX_CAPACITY
        );
        let len = capacity as usize;
        Self {
            capacity,
            slot_count: 0,
            alive_count: 0,
            bounds_min: Vec2::ZERO,
            bounds_max: Vec2::ZERO,
            dropped_spawns: 0,
            free_list: VecDeque::new(),
            alive: vec![false; len],
            generation: vec![0; len],
            unit: vec![0; len],
            unit_id: vec![UnitId::INVALID; len],
            bullet_type: vec![0; len],
            program: vec![0; len],
            flags: vec![BulletFlags(0); len],
            cascade: vec![0; len],
            position: vec![Vec2::ZERO; len],
            previous_position: vec![Vec2::ZERO; len],
            velocity: vec![Vec2::ZERO; len],
            angle: vec![0.0; len],
            speed: vec![0.0; len],
            age: vec![0; len],
            state: vec![[0.0; 4]; len],
            events: Vec::new(),
            events_tick: 0,
        }
    }

    /// Total number of allocated slots.
    #[must_use]
    pub const fn capacity(&self) -> u32 {
        self.capacity
    }

    /// Number of currently live bullets.
    #[must_use]
    pub const fn len(&self) -> u32 {
        self.alive_count
    }

    /// Whether there are no live bullets.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.alive_count == 0
    }

    /// High-water mark of ever-used slots.
    #[must_use]
    pub const fn slot_count(&self) -> u32 {
        self.slot_count
    }

    /// Spawns deterministically discarded by a caller that has no `Result` to report to.
    ///
    /// A direct call to `BulletPool::spawn` never counts here: it surfaces a full pool as
    /// [`SigilError::PoolFull`] instead of dropping silently, so it stays `0` unless
    /// `sigil.emit` (`crate::systems`, §11.3/§11.6) drops a volley shot because the pool was full,
    /// which it reports through `BulletPool::record_dropped_spawn` instead of a `Result` no
    /// caller could observe.
    #[must_use]
    pub const fn dropped_spawns(&self) -> u64 {
        self.dropped_spawns
    }

    /// Whether `id` still refers to a live bullet.
    #[must_use]
    pub fn is_alive(&self, id: BulletId) -> bool {
        (id.index < self.slot_count)
            && self.alive[id.index as usize]
            && self.generation[id.index as usize] == id.generation
    }

    /// Read-only view of a live bullet, or `None` for a dead or unknown handle.
    #[must_use]
    pub fn get(&self, id: BulletId) -> Option<BulletRef<'_>> {
        self.is_alive(id).then_some(BulletRef {
            pool: self,
            index: id.index as usize,
        })
    }

    /// Iterates every live bullet in ascending slot order.
    pub fn iter(&self) -> impl Iterator<Item = BulletRef<'_>> {
        (0..self.slot_count as usize)
            .filter(|&index| self.alive[index])
            .map(move |index| BulletRef { pool: self, index })
    }

    /// Column view over slots `0..slot_count`.
    #[must_use]
    pub fn columns(&self) -> BulletColumns<'_> {
        self.columns_for(0..self.slot_count as usize)
    }

    fn columns_for(&self, range: Range<usize>) -> BulletColumns<'_> {
        BulletColumns {
            alive: &self.alive[range.clone()],
            generation: &self.generation[range.clone()],
            unit: &self.unit[range.clone()],
            bullet_type: &self.bullet_type[range.clone()],
            program: &self.program[range.clone()],
            flags: &self.flags[range.clone()],
            cascade: &self.cascade[range.clone()],
            position: &self.position[range.clone()],
            previous_position: &self.previous_position[range.clone()],
            velocity: &self.velocity[range.clone()],
            angle: &self.angle[range.clone()],
            speed: &self.speed[range.clone()],
            age: &self.age[range.clone()],
            state: &self.state[range],
        }
    }

    /// Splits slots `0..slot_count` into fixed-size blocks
    /// (`grimoire_ecs::QUERY_BLOCK_SIZE`, matching `grimoire_ecs::slice_block_ranges`).
    pub fn blocks(&self) -> impl ExactSizeIterator<Item = PoolBlock<'_>> {
        slice_block_ranges(self.slot_count as usize)
            .enumerate()
            .map(move |(index, range)| PoolBlock {
                index,
                slots: range.clone(),
                columns: self.columns_for(range),
            })
    }

    /// Takes slots `0..slot_count` out as mutable, data-parallel [`PoolUpdateBlock`]s for
    /// `sigil.update` (contract §11.7): every needed column is split with `split_at_mut` at
    /// exactly the [`slice_block_ranges`] boundaries, so the result matches [`BulletPool::blocks`]
    /// block-for-block but grants each block write access to its own slots only.
    ///
    /// # Panics
    ///
    /// Only on an internal invariant violation (every split below must yield the same number of
    /// blocks, since every column has the same `slot_count`); never on caller input.
    pub(crate) fn update_blocks_mut(&mut self) -> Vec<PoolUpdateBlock<'_>> {
        let slot_count = self.slot_count as usize;
        let ranges: Vec<Range<usize>> = slice_block_ranges(slot_count).collect();

        let mut alive = split_ref_blocks(&self.alive[..slot_count]).into_iter();
        let mut unit = split_ref_blocks(&self.unit[..slot_count]).into_iter();
        let mut bullet_type = split_mut_blocks(&mut self.bullet_type[..slot_count]).into_iter();
        let mut program = split_ref_blocks(&self.program[..slot_count]).into_iter();
        let mut flags = split_mut_blocks(&mut self.flags[..slot_count]).into_iter();
        let mut cascade = split_ref_blocks(&self.cascade[..slot_count]).into_iter();
        let mut position = split_mut_blocks(&mut self.position[..slot_count]).into_iter();
        let mut previous_position =
            split_mut_blocks(&mut self.previous_position[..slot_count]).into_iter();
        let mut velocity = split_mut_blocks(&mut self.velocity[..slot_count]).into_iter();
        let mut angle = split_mut_blocks(&mut self.angle[..slot_count]).into_iter();
        let mut speed = split_mut_blocks(&mut self.speed[..slot_count]).into_iter();
        let mut age = split_mut_blocks(&mut self.age[..slot_count]).into_iter();
        let mut state = split_mut_blocks(&mut self.state[..slot_count]).into_iter();

        ranges
            .into_iter()
            .enumerate()
            .map(|(index, slots)| PoolUpdateBlock {
                index,
                slots,
                alive: alive.next().expect("column block count must match"),
                unit: unit.next().expect("column block count must match"),
                bullet_type: bullet_type.next().expect("column block count must match"),
                program: program.next().expect("column block count must match"),
                flags: flags.next().expect("column block count must match"),
                cascade: cascade.next().expect("column block count must match"),
                position: position.next().expect("column block count must match"),
                previous_position: previous_position
                    .next()
                    .expect("column block count must match"),
                velocity: velocity.next().expect("column block count must match"),
                angle: angle.next().expect("column block count must match"),
                speed: speed.next().expect("column block count must match"),
                age: age.next().expect("column block count must match"),
                state: state.next().expect("column block count must match"),
            })
            .collect()
    }

    /// Resets [`BulletPool::events`]/[`BulletPool::events_tick`] for the start of a new tick
    /// (`sigil.begin`, contract §11.6).
    pub(crate) fn begin_tick(&mut self, tick: u64) {
        self.events.clear();
        self.events_tick = tick;
    }

    /// Records a spawn a tick-phase system discarded because it had no `Result` channel to
    /// report it on (`sigil.emit`, contract §11.3): the pool is otherwise unchanged. A direct
    /// caller of [`BulletPool::spawn`] must instead handle `Err(SigilError::PoolFull)` itself and
    /// must not call this (contract §11.3's WP5.1 clarification).
    pub(crate) fn record_dropped_spawn(&mut self) {
        self.dropped_spawns += 1;
    }

    /// Sets the simulation bounds `sigil.update` checks (`install`'s `SigilConfig`, contract
    /// §11.6).
    pub(crate) fn set_bounds(&mut self, min: Vec2, max: Vec2) {
        self.bounds_min = min;
        self.bounds_max = max;
    }

    /// Current simulation bounds.
    pub(crate) const fn bounds(&self) -> (Vec2, Vec2) {
        (self.bounds_min, self.bounds_max)
    }

    /// Despawns slot `index`, given a [`PendingDespawn`] collected by a `sigil.update` block
    /// (`sigil.resolve`, contract §11.6). A no-op if the slot is not alive any more: `index`
    /// always comes from the same tick's `sigil.update`, in which no other despawn of that slot
    /// can have happened yet, so this is a defensive guard against a future bug, not a reachable
    /// path today.
    pub(crate) fn despawn_pending(&mut self, index: u32, cause: DespawnCause) {
        if self.alive[index as usize] {
            self.despawn_slot(index, cause);
        }
    }

    /// Despawn events recorded since the pool was created or last cleared by `sigil.begin`
    /// (`BulletPool::begin_tick`, contract §11.3/§11.6; a pool never installed into a `Simulation`
    /// simply accumulates them, as this crate's own unit tests do).
    #[must_use]
    pub fn events(&self) -> &[BulletEvent] {
        &self.events
    }

    /// Tick at which [`BulletPool::events`] was last reset by `sigil.begin`
    /// (`BulletPool::begin_tick`); `0` for a pool that was never installed into a `Simulation`.
    #[must_use]
    pub const fn events_tick(&self) -> u64 {
        self.events_tick
    }

    /// Spawns one bullet.
    ///
    /// Validates the unit, bullet-type index, program index, cascade depth and finiteness of
    /// `request`'s position/angle/speed, in that order, before touching any pool state: on any
    /// error the pool is completely unchanged, which is asserted by hash-equality in this crate's
    /// tests.
    ///
    /// **`request.program` numbering** (WP5.1, `crate::runtime::resolve_program`): `0` means "no
    /// program" (`BulletSpawn::new`'s doc comment); a non-zero value `p` addresses
    /// `unit.programs()[p - 1]`, one-based so that `0` stays free as the sentinel and every
    /// compiled program is still reachable — unlike a direct `p` mapping, which would leave
    /// `programs()[0]` permanently unreachable. Contract §11.3 fixes the *validity* check as
    /// `program == 0 || program < program_count()`; read against this one-based scheme that
    /// bound is off by one (it rejects the otherwise-valid `program == program_count()`, the last
    /// program), so this method checks `program > program_count()` instead — a clarification
    /// (V-20), not a behaviour change for any caller that only ever used `program` values already
    /// accepted before (`0` or `1..program_count()`); see the WP5.1 report for the wording.
    ///
    /// # Errors
    ///
    /// - [`SigilError::UnknownUnit`] if `request.unit` is not loaded.
    /// - [`SigilError::BulletTypeOutOfRange`] if `request.bullet_type` has no matching record.
    /// - [`SigilError::ProgramOutOfRange`] if `request.program` is neither `0` nor a valid program.
    /// - [`SigilError::CascadeTooDeep`] if `request.cascade` exceeds
    ///   [`SigilUnit::MAX_CASCADE_DEPTH`].
    /// - [`SigilError::NonFinite`] if position, angle or speed is not finite.
    /// - [`SigilError::PoolFull`] if there is no free or fresh slot.
    pub fn spawn(
        &mut self,
        content: &SigilContent,
        request: BulletSpawn,
    ) -> Result<BulletId, SigilError> {
        let library = content.library();
        let unit_index = library
            .unit_index(request.unit)
            .ok_or(SigilError::UnknownUnit(request.unit))?;
        let sigil_unit = &library.units()[unit_index as usize];

        if usize::from(request.bullet_type) >= sigil_unit.bullet_types().len() {
            return Err(SigilError::BulletTypeOutOfRange {
                unit: request.unit,
                bullet_type: request.bullet_type,
            });
        }
        if request.program != 0 && request.program > sigil_unit.program_count() {
            return Err(SigilError::ProgramOutOfRange {
                unit: request.unit,
                program: request.program,
            });
        }
        if request.cascade > SigilUnit::MAX_CASCADE_DEPTH {
            return Err(SigilError::CascadeTooDeep {
                depth: request.cascade,
            });
        }
        if !request.position.x.is_finite()
            || !request.position.y.is_finite()
            || !request.angle.is_finite()
            || !request.speed.is_finite()
        {
            return Err(SigilError::NonFinite);
        }

        // Every remaining step is infallible except running out of slots, and `PoolFull` is
        // returned before any mutation, so the pool stays byte-identical on every error path.
        let (index, generation) = if let Some(index) = self.free_list.pop_front() {
            (index, self.generation[index as usize])
        } else if self.slot_count < self.capacity {
            let index = self.slot_count;
            self.slot_count += 1;
            (index, 0u32)
        } else {
            return Err(SigilError::PoolFull);
        };

        let i = index as usize;
        let velocity = Vec2::from_angle(request.angle) * request.speed;
        self.alive[i] = true;
        self.generation[i] = generation;
        self.unit[i] = unit_index;
        self.unit_id[i] = request.unit;
        self.bullet_type[i] = request.bullet_type;
        self.program[i] = request.program;
        self.flags[i] = sigil_unit.bullet_types()[request.bullet_type as usize].flags;
        self.cascade[i] = request.cascade;
        self.position[i] = request.position;
        self.previous_position[i] = request.position;
        self.velocity[i] = velocity;
        self.angle[i] = request.angle;
        self.speed[i] = request.speed;
        self.age[i] = 0;
        self.state[i] = [0.0; 4];
        self.alive_count += 1;

        debug_assert!(
            self.position[i].x.is_finite() && self.position[i].y.is_finite(),
            "spawn already validated finiteness"
        );
        debug_assert!(
            self.velocity[i].x.is_finite() && self.velocity[i].y.is_finite(),
            "velocity computed from finite angle/speed must be finite"
        );

        Ok(BulletId { index, generation })
    }

    /// Despawns one bullet, recording a [`BulletEvent`].
    ///
    /// Returns `false` without recording an event if `id` is not currently alive (already
    /// despawned, or stale after its slot was reused).
    pub fn despawn(&mut self, id: BulletId, cause: DespawnCause) -> bool {
        if !self.is_alive(id) {
            return false;
        }
        self.despawn_slot(id.index, cause);
        true
    }

    /// Despawns every live bullet matching `filter`, in ascending slot order, applying immediately
    /// and recording one [`BulletEvent`] per despawned bullet. Returns the number despawned.
    pub fn clear(
        &mut self,
        content: &SigilContent,
        filter: &ClearFilter,
        cause: DespawnCause,
    ) -> u32 {
        let target_index = match filter {
            ClearFilter::Unit(unit) | ClearFilter::Type(unit, _) => {
                content.library().unit_index(*unit)
            }
            ClearFilter::All | ClearFilter::AnyFlags(_) => None,
        };

        let mut despawned = 0u32;
        for index in 0..self.slot_count {
            let i = index as usize;
            if !self.alive[i] {
                continue;
            }
            let matches = match filter {
                ClearFilter::All => true,
                ClearFilter::Unit(_) => target_index == Some(self.unit[i]),
                ClearFilter::Type(_, bullet_type) => {
                    target_index == Some(self.unit[i]) && self.bullet_type[i] == *bullet_type
                }
                ClearFilter::AnyFlags(mask) => (self.flags[i].0 & mask.0) != 0,
            };
            if matches {
                self.despawn_slot(index, cause);
                despawned += 1;
            }
        }
        despawned
    }

    /// Frees slot `index`, recording an event and either recycling it (bumped generation pushed to
    /// the free list) or retiring it forever if the generation would overflow — exactly the ECS
    /// entity allocator's rule (contract §7/§11.3).
    fn despawn_slot(&mut self, index: u32, cause: DespawnCause) {
        let i = index as usize;
        self.alive[i] = false;
        self.alive_count -= 1;
        self.events.push(BulletEvent {
            id: BulletId {
                index,
                generation: self.generation[i],
            },
            unit: self.unit_id[i],
            bullet_type: self.bullet_type[i],
            position: self.position[i],
            cause,
        });
        if let Some(next) = self.generation[i].checked_add(1) {
            self.generation[i] = next;
            self.free_list.push_back(index);
        }
    }
}

impl StableHash for BulletPool {
    /// Feeds, in order (contract §11.3): `capacity`, `slot_count`, `bounds_min`, `bounds_max`,
    /// `dropped_spawns`; per slot the generation and alive flag; the free list's length then its
    /// indices in reuse order; per live slot the [`BulletColumns`] fields from `unit` onward; then
    /// `events_tick`, the event count and the events.
    fn stable_hash(&self, hasher: &mut StableHasher) {
        hasher.write_u32(self.capacity);
        hasher.write_u32(self.slot_count);
        self.bounds_min.stable_hash(hasher);
        self.bounds_max.stable_hash(hasher);
        hasher.write_u64(self.dropped_spawns);

        for i in 0..self.slot_count as usize {
            hasher.write_u32(self.generation[i]);
            hasher.write_bool(self.alive[i]);
        }

        hasher.write_usize(self.free_list.len());
        for &index in &self.free_list {
            hasher.write_u32(index);
        }

        for i in 0..self.slot_count as usize {
            if !self.alive[i] {
                continue;
            }
            hasher.write_u16(self.unit[i]);
            hasher.write_u16(self.bullet_type[i]);
            hasher.write_u16(self.program[i]);
            self.flags[i].stable_hash(hasher);
            hasher.write_u8(self.cascade[i]);
            self.position[i].stable_hash(hasher);
            self.previous_position[i].stable_hash(hasher);
            self.velocity[i].stable_hash(hasher);
            hasher.write_f32(self.angle[i]);
            hasher.write_f32(self.speed[i]);
            hasher.write_u32(self.age[i]);
            self.state[i].stable_hash(hasher);
        }

        hasher.write_u64(self.events_tick);
        self.events.stable_hash(hasher);
    }
}

#[cfg(test)]
mod tests {
    use grimoire_core::hash_of;
    use grimoire_core::math::dmath;

    use super::*;
    use crate::content::BulletType;
    use crate::test_support::{build_content, build_simple_content, build_unit, plain_bullet_type};

    fn spawn_at(pool: &mut BulletPool, content: &SigilContent, x: f32) -> BulletId {
        pool.spawn(
            content,
            BulletSpawn::new(UnitId(1), 0, Vec2::new(x, 0.0), 0.0, 1.0),
        )
        .expect("spawn must succeed")
    }

    fn two_unit_content() -> SigilContent {
        let bullet_types = vec![plain_bullet_type(), plain_bullet_type()];
        let unit_a = build_unit(1, &bullet_types, 0);
        let unit_b = build_unit(2, &bullet_types, 0);
        build_content(vec![unit_a, unit_b])
    }

    #[test]
    fn bullet_id_bit_round_trip_and_ordering() {
        let id = BulletId {
            index: 7,
            generation: 3,
        };
        assert_eq!(BulletId::from_bits(id.to_bits()), id);
        assert_eq!(id.index(), 7);
        assert_eq!(id.generation(), 3);
        assert!(
            BulletId {
                index: 1,
                generation: 9
            } < BulletId {
                index: 2,
                generation: 0
            }
        );
        assert!(
            BulletId {
                index: 1,
                generation: 0
            } < BulletId {
                index: 1,
                generation: 1
            }
        );
        assert_eq!(id.to_string(), "7v3");
    }

    #[test]
    fn bounds_are_part_of_the_pool_hash() {
        // Contract §11.3 freezes `bounds_min`/`bounds_max` right after `slot_count` in the hash
        // order; `set_bounds` is `install`'s writer (§11.6), exercised end-to-end in
        // `crate::systems`'s own tests. This test only proves the hash-layout claim.
        let mut pool = BulletPool::with_capacity(4);
        let before = hash_of(&pool);

        pool.set_bounds(Vec2::new(-10.0, -20.0), Vec2::new(10.0, 20.0));

        assert_ne!(
            hash_of(&pool),
            before,
            "bounds_min/bounds_max must be fed into the pool hash (contract §11.3)"
        );
    }

    #[test]
    fn default_pool_is_empty_and_allocates_nothing() {
        let pool = BulletPool::default();
        assert_eq!(pool.capacity(), 0);
        assert_eq!(pool.slot_count(), 0);
        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());
        assert_eq!(pool.dropped_spawns(), 0);
        assert!(pool.events().is_empty());
    }

    #[test]
    #[should_panic(expected = "exceeds MAX_CAPACITY")]
    fn with_capacity_panics_over_max() {
        let _ = BulletPool::with_capacity(BulletPool::MAX_CAPACITY + 1);
    }

    #[test]
    fn spawn_writes_expected_slot_fields() {
        let content = build_simple_content(1, 0, 0);
        let mut pool = BulletPool::with_capacity(4);
        let angle = dmath::FRAC_PI_2;
        let speed = 2.0;
        let position = Vec2::new(3.0, 4.0);
        let id = pool
            .spawn(
                &content,
                BulletSpawn::new(UnitId(1), 0, position, angle, speed),
            )
            .unwrap();
        let bullet = pool.get(id).unwrap();
        assert_eq!(bullet.position(), position);
        assert_eq!(bullet.previous_position(), position);
        assert_eq!(bullet.age(), 0);
        let expected_velocity = Vec2::from_angle(angle) * speed;
        assert!((bullet.velocity().x - expected_velocity.x).abs() < 1e-5);
        assert!((bullet.velocity().y - expected_velocity.y).abs() < 1e-5);
        assert_eq!(bullet.unit_index(), 0);
        assert_eq!(bullet.bullet_type(), 0);
        assert_eq!(pool.len(), 1);
        assert_eq!(pool.slot_count(), 1);
    }

    #[test]
    fn despawn_marks_dead_and_records_an_event() {
        let content = build_simple_content(1, 0, 0);
        let mut pool = BulletPool::with_capacity(4);
        let id = spawn_at(&mut pool, &content, 1.0);
        assert!(pool.despawn(id, DespawnCause::External));
        assert!(!pool.is_alive(id));
        assert!(pool.get(id).is_none());
        assert_eq!(pool.len(), 0);
        assert_eq!(pool.events().len(), 1);
        assert_eq!(pool.events()[0].id, id);
        assert_eq!(pool.events()[0].unit, UnitId(1));
        assert_eq!(pool.events()[0].cause, DespawnCause::External);
    }

    #[test]
    fn despawn_of_unknown_or_already_dead_handle_returns_false() {
        let content = build_simple_content(1, 0, 0);
        let mut pool = BulletPool::with_capacity(4);
        let unknown = BulletId {
            index: 0,
            generation: 0,
        };
        assert!(!pool.despawn(unknown, DespawnCause::External));

        let id = spawn_at(&mut pool, &content, 0.0);
        assert!(pool.despawn(id, DespawnCause::External));
        assert!(
            !pool.despawn(id, DespawnCause::External),
            "double despawn must fail"
        );
    }

    #[test]
    fn fifo_reuse_bumps_generation_oldest_first() {
        let content = build_simple_content(1, 0, 0);
        let mut pool = BulletPool::with_capacity(4);
        let a = spawn_at(&mut pool, &content, 0.0);
        let b = spawn_at(&mut pool, &content, 1.0);
        let c = spawn_at(&mut pool, &content, 2.0);

        assert!(pool.despawn(b, DespawnCause::External));
        assert!(pool.despawn(a, DespawnCause::External));

        let first = spawn_at(&mut pool, &content, 3.0);
        let second = spawn_at(&mut pool, &content, 4.0);
        assert_eq!(
            first,
            BulletId {
                index: b.index(),
                generation: 1
            },
            "free list must be FIFO: b was freed before a"
        );
        assert_eq!(
            second,
            BulletId {
                index: a.index(),
                generation: 1
            }
        );
        assert!(pool.is_alive(c));
        assert!(!pool.is_alive(a));
        assert!(!pool.is_alive(b));

        let fresh = spawn_at(&mut pool, &content, 5.0);
        assert_eq!(fresh.index(), 3);
        assert_eq!(fresh.generation(), 0);
    }

    #[test]
    fn capacity_exhaustion_returns_pool_full_without_side_effects() {
        let content = build_simple_content(1, 0, 0);
        let mut pool = BulletPool::with_capacity(2);
        spawn_at(&mut pool, &content, 0.0);
        spawn_at(&mut pool, &content, 1.0);
        let before = hash_of(&pool);

        let result = pool.spawn(
            &content,
            BulletSpawn::new(UnitId(1), 0, Vec2::ZERO, 0.0, 1.0),
        );
        assert_eq!(result, Err(SigilError::PoolFull));
        assert_eq!(
            hash_of(&pool),
            before,
            "a rejected spawn must not change any state"
        );
    }

    #[test]
    fn failed_spawns_leave_the_pool_hash_unchanged() {
        let content = build_simple_content(2, 0, 2);
        let mut pool = BulletPool::with_capacity(4);
        spawn_at(&mut pool, &content, 0.0);
        let before = hash_of(&pool);

        let unknown_unit = pool.spawn(
            &content,
            BulletSpawn::new(UnitId(99), 0, Vec2::ZERO, 0.0, 1.0),
        );
        assert!(matches!(
            unknown_unit,
            Err(SigilError::UnknownUnit(UnitId(99)))
        ));

        let bad_bullet_type = pool.spawn(
            &content,
            BulletSpawn::new(UnitId(1), 5, Vec2::ZERO, 0.0, 1.0),
        );
        assert!(matches!(
            bad_bullet_type,
            Err(SigilError::BulletTypeOutOfRange { bullet_type: 5, .. })
        ));

        let bad_program = pool.spawn(
            &content,
            BulletSpawn::new(UnitId(1), 0, Vec2::ZERO, 0.0, 1.0).with_program(9),
        );
        assert!(matches!(
            bad_program,
            Err(SigilError::ProgramOutOfRange { program: 9, .. })
        ));

        let too_deep = pool.spawn(
            &content,
            BulletSpawn::new(UnitId(1), 0, Vec2::ZERO, 0.0, 1.0)
                .with_cascade(SigilUnit::MAX_CASCADE_DEPTH + 1),
        );
        assert!(matches!(too_deep, Err(SigilError::CascadeTooDeep { .. })));

        let non_finite = pool.spawn(
            &content,
            BulletSpawn::new(UnitId(1), 0, Vec2::new(f32::NAN, 0.0), 0.0, 1.0),
        );
        assert!(matches!(non_finite, Err(SigilError::NonFinite)));

        assert_eq!(
            hash_of(&pool),
            before,
            "every rejected spawn must leave the pool untouched"
        );
    }

    /// `spawn`'s doc comment (WP5.1 clarification): `program` is one-based when non-zero, so the
    /// *last* compiled program is addressed by `program == program_count()`, not
    /// `program_count() - 1`. `program_count() + 1` still errors.
    #[test]
    fn program_equal_to_program_count_is_the_last_program_not_out_of_range() {
        let content = build_simple_content(1, 0, 2);
        let mut pool = BulletPool::with_capacity(2);

        let last = pool.spawn(
            &content,
            BulletSpawn::new(UnitId(1), 0, Vec2::ZERO, 0.0, 1.0).with_program(2),
        );
        assert!(last.is_ok(), "program == program_count() must be valid");

        let one_past = pool.spawn(
            &content,
            BulletSpawn::new(UnitId(1), 0, Vec2::ZERO, 0.0, 1.0).with_program(3),
        );
        assert!(matches!(
            one_past,
            Err(SigilError::ProgramOutOfRange { program: 3, .. })
        ));
    }

    #[test]
    fn clear_all_despawns_every_live_bullet() {
        let content = two_unit_content();
        let mut pool = BulletPool::with_capacity(8);
        for _ in 0..3 {
            pool.spawn(
                &content,
                BulletSpawn::new(UnitId(1), 0, Vec2::ZERO, 0.0, 1.0),
            )
            .unwrap();
        }
        for _ in 0..2 {
            pool.spawn(
                &content,
                BulletSpawn::new(UnitId(2), 0, Vec2::ZERO, 0.0, 1.0),
            )
            .unwrap();
        }
        assert_eq!(pool.len(), 5);
        let despawned = pool.clear(&content, &ClearFilter::All, DespawnCause::Clear);
        assert_eq!(despawned, 5);
        assert!(pool.is_empty());
        assert_eq!(pool.events().len(), 5);
    }

    #[test]
    fn clear_by_unit_only_despawns_that_unit() {
        let content = two_unit_content();
        let mut pool = BulletPool::with_capacity(8);
        let a = pool
            .spawn(
                &content,
                BulletSpawn::new(UnitId(1), 0, Vec2::ZERO, 0.0, 1.0),
            )
            .unwrap();
        let b = pool
            .spawn(
                &content,
                BulletSpawn::new(UnitId(2), 0, Vec2::ZERO, 0.0, 1.0),
            )
            .unwrap();
        let despawned = pool.clear(&content, &ClearFilter::Unit(UnitId(1)), DespawnCause::Clear);
        assert_eq!(despawned, 1);
        assert!(!pool.is_alive(a));
        assert!(pool.is_alive(b));
    }

    #[test]
    fn clear_by_type_matches_unit_and_bullet_type() {
        let content = two_unit_content();
        let mut pool = BulletPool::with_capacity(8);
        let a0 = pool
            .spawn(
                &content,
                BulletSpawn::new(UnitId(1), 0, Vec2::ZERO, 0.0, 1.0),
            )
            .unwrap();
        let a1 = pool
            .spawn(
                &content,
                BulletSpawn::new(UnitId(1), 1, Vec2::ZERO, 0.0, 1.0),
            )
            .unwrap();
        let b0 = pool
            .spawn(
                &content,
                BulletSpawn::new(UnitId(2), 0, Vec2::ZERO, 0.0, 1.0),
            )
            .unwrap();
        let despawned = pool.clear(
            &content,
            &ClearFilter::Type(UnitId(1), 0),
            DespawnCause::Clear,
        );
        assert_eq!(despawned, 1);
        assert!(!pool.is_alive(a0));
        assert!(pool.is_alive(a1));
        assert!(pool.is_alive(b0));
    }

    #[test]
    fn clear_by_any_flags_matches_overlapping_bits() {
        let bullet_types = [
            BulletType {
                flags: BulletFlags::SMASHABLE,
                ..plain_bullet_type()
            },
            BulletType {
                flags: BulletFlags::GRAZEABLE,
                ..plain_bullet_type()
            },
            BulletType {
                flags: BulletFlags(0),
                ..plain_bullet_type()
            },
        ];
        let unit = build_unit(1, &bullet_types, 0);
        let content = build_content(vec![unit]);
        let mut pool = BulletPool::with_capacity(8);
        let smash = pool
            .spawn(
                &content,
                BulletSpawn::new(UnitId(1), 0, Vec2::ZERO, 0.0, 1.0),
            )
            .unwrap();
        let graze = pool
            .spawn(
                &content,
                BulletSpawn::new(UnitId(1), 1, Vec2::ZERO, 0.0, 1.0),
            )
            .unwrap();
        let plain = pool
            .spawn(
                &content,
                BulletSpawn::new(UnitId(1), 2, Vec2::ZERO, 0.0, 1.0),
            )
            .unwrap();

        let mask = BulletFlags(BulletFlags::SMASHABLE.0 | BulletFlags::GRAZEABLE.0);
        let despawned = pool.clear(&content, &ClearFilter::AnyFlags(mask), DespawnCause::Clear);
        assert_eq!(despawned, 2);
        assert!(!pool.is_alive(smash));
        assert!(!pool.is_alive(graze));
        assert!(pool.is_alive(plain));
    }

    #[test]
    fn clear_by_unknown_unit_despawns_nothing() {
        let content = build_simple_content(1, 0, 0);
        let mut pool = BulletPool::with_capacity(4);
        pool.spawn(
            &content,
            BulletSpawn::new(UnitId(1), 0, Vec2::ZERO, 0.0, 1.0),
        )
        .unwrap();
        let despawned = pool.clear(
            &content,
            &ClearFilter::Unit(UnitId(999)),
            DespawnCause::Clear,
        );
        assert_eq!(despawned, 0);
        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn iter_get_and_columns_agree() {
        let content = build_simple_content(1, 0, 0);
        let mut pool = BulletPool::with_capacity(4);
        let a = spawn_at(&mut pool, &content, 0.0);
        let b = spawn_at(&mut pool, &content, 1.0);
        pool.despawn(a, DespawnCause::External);
        let c = spawn_at(&mut pool, &content, 2.0); // reuses a's slot

        let iter_ids: Vec<BulletId> = pool.iter().map(|bullet| bullet.id()).collect();
        assert_eq!(iter_ids.len(), 2);
        assert!(iter_ids.contains(&b));
        assert!(iter_ids.contains(&c));

        let columns = pool.columns();
        assert_eq!(columns.alive.len(), pool.slot_count() as usize);
        for (index, &alive) in columns.alive.iter().enumerate() {
            let id = BulletId {
                index: index as u32,
                generation: columns.generation[index],
            };
            assert_eq!(alive, pool.is_alive(id));
        }
        assert!(pool.get(b).is_some());
        assert!(pool.get(a).is_none());
    }

    #[test]
    fn blocks_match_slice_block_ranges() {
        let content = build_simple_content(1, 0, 0);
        let count = grimoire_ecs::QUERY_BLOCK_SIZE + 6;
        let mut pool = BulletPool::with_capacity(count as u32);
        for i in 0..count {
            spawn_at(&mut pool, &content, i as f32);
        }
        let expected: Vec<Range<usize>> = slice_block_ranges(pool.slot_count() as usize).collect();
        assert_eq!(
            expected.len(),
            2,
            "test setup should exercise more than one block"
        );
        let blocks: Vec<PoolBlock<'_>> = pool.blocks().collect();
        assert_eq!(blocks.len(), expected.len());
        for (block_index, (block, range)) in blocks.iter().zip(expected.iter()).enumerate() {
            assert_eq!(block.index(), block_index);
            assert_eq!(block.slots(), *range);
            assert_eq!(block.columns().alive.len(), range.len());
        }
    }

    #[test]
    fn clone_is_an_independent_snapshot() {
        let content = build_simple_content(1, 0, 0);
        let mut pool = BulletPool::with_capacity(8);
        let a = spawn_at(&mut pool, &content, 0.0);
        let snapshot = pool.clone();
        let snapshot_hash = hash_of(&snapshot);

        spawn_at(&mut pool, &content, 1.0);
        pool.despawn(a, DespawnCause::External);

        assert_eq!(
            hash_of(&snapshot),
            snapshot_hash,
            "the snapshot must not observe later mutation of the original"
        );
        assert_ne!(hash_of(&pool), snapshot_hash);

        // Replaying the same operations from scratch must reach the same state.
        let mut replay = BulletPool::with_capacity(8);
        let replay_a = spawn_at(&mut replay, &content, 0.0);
        spawn_at(&mut replay, &content, 1.0);
        replay.despawn(replay_a, DespawnCause::External);
        assert_eq!(hash_of(&replay), hash_of(&pool));
    }

    /// Frozen reference hash for the exact `StableHash` field order documented on
    /// `impl StableHash for BulletPool`. If this fails after an *intentional* change to that
    /// order, recompute it (the assertion prints the actual value) and update this constant.
    ///
    /// Updated for the `bounds_min`/`bounds_max` fix (review finding on PR #3): the previous
    /// value (`0x0733_8e7e_2316_04b7`) was computed from a hash order that omitted those two
    /// fields entirely, so it never actually matched the frozen contract §11.3 layout.
    const GOLDEN_POOL_HASH: u64 = 0x9672_671e_4036_f68c;

    #[test]
    fn golden_pool_hash() {
        let content = build_simple_content(2, 1, 2);
        let mut pool = BulletPool::with_capacity(4);
        let a = pool
            .spawn(
                &content,
                BulletSpawn::new(UnitId(1), 0, Vec2::new(1.0, 2.0), 0.0, 3.0),
            )
            .unwrap();
        pool.spawn(
            &content,
            BulletSpawn::new(UnitId(1), 1, Vec2::new(-1.0, 0.5), dmath::FRAC_PI_2, 2.0)
                .with_program(1),
        )
        .unwrap();
        pool.despawn(a, DespawnCause::Lifetime);
        pool.spawn(
            &content,
            BulletSpawn::new(UnitId(1), 0, Vec2::ZERO, 1.0, 1.0).with_cascade(1),
        )
        .unwrap();

        let hash = hash_of(&pool);
        assert_eq!(
            hash, GOLDEN_POOL_HASH,
            "pool hash layout changed; if intentional, update GOLDEN_POOL_HASH to {hash:#018x}"
        );
    }
}
