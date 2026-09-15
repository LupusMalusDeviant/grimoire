//! `grimoire_sigil` runtime v1 (contract §11).

use std::collections::BTreeMap;
use std::fmt;
use std::ops::Range;
use std::sync::Arc;

use grimoire_core::{StableHash, StableHasher, Vec2, impl_stable_hash};
use grimoire_ecs::{Entity, World, system_fn};
use grimoire_ecs_p1::{run_blocks, slice_block_ranges};
use grimoire_sim::{SimRng, SimSeed, SimSnapshot, Simulation, Tick, derive_block_rng};
use grimoire_sim_p1::{ContentManifestHash, SimSnapshotResource, SimulationP1};

// ---- §11.1 ------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct UnitId(pub u64);

impl fmt::Display for UnitId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}
impl StableHash for UnitId {
    fn stable_hash(&self, hasher: &mut StableHasher) {
        hasher.write_u64(self.0);
    }
}

#[derive(Clone, Debug)]
pub struct SigilUnit {
    bytes: Vec<u8>,
    bullet_types: Vec<BulletType>,
    behavior_refs: Vec<BehaviorId>,
}

impl PartialEq for SigilUnit {
    fn eq(&self, other: &Self) -> bool {
        self.bytes == other.bytes
    }
}
impl Eq for SigilUnit {}

impl SigilUnit {
    pub const MAGIC: [u8; 8] = *b"GRIMSIGL";
    pub const FORMAT_VERSION: u32 = 1;
    pub const HEADER_LEN: usize = 40;
    pub const MAX_UNIT_BYTES: usize = 8 * 1024 * 1024;
    pub const MAX_CASCADE_DEPTH: u8 = 3;

    pub fn from_bytes(bytes: &[u8]) -> Result<SigilUnit, UnitError> {
        let _ = bytes;
        unimplemented!()
    }
    pub fn to_bytes(&self) -> Vec<u8> {
        self.bytes.clone()
    }
    pub fn id(&self) -> UnitId {
        unimplemented!()
    }
    pub fn content_hash(&self) -> u64 {
        unimplemented!()
    }
    pub fn bullet_types(&self) -> &[BulletType] {
        &self.bullet_types
    }
    pub fn emitter_count(&self) -> u16 {
        unimplemented!()
    }
    pub fn program_count(&self) -> u16 {
        unimplemented!()
    }
    pub fn behavior_refs(&self) -> &[BehaviorId] {
        &self.behavior_refs
    }
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum UnitError {
    #[error("unexpected end")]
    UnexpectedEnd {
        offset: u64,
        needed: u64,
        available: u64,
    },
    #[error("bad magic")]
    BadMagic,
    #[error("unsupported version {0}")]
    UnsupportedVersion(u32),
    #[error("reserved flags {0}")]
    ReservedFlags(u32),
    #[error("payload length")]
    PayloadLength { declared: u64, actual: u64 },
    #[error("content hash")]
    ContentHash { declared: u64, computed: u64 },
    #[error("unknown section")]
    UnknownSection { kind: u32 },
    #[error("section layout")]
    SectionLayout { kind: u32 },
    #[error("limit")]
    Limit {
        what: &'static str,
        value: u64,
        max: u64,
    },
    #[error("index out of range")]
    IndexOutOfRange {
        what: &'static str,
        index: u64,
        len: u64,
    },
    #[error("non-finite")]
    NonFinite { offset: u64 },
    #[error("cascade too deep")]
    CascadeTooDeep { depth: u8 },
    #[error("non-canonical")]
    NonCanonical { offset: u64 },
}

// ---- §11.2 ------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BulletType {
    pub visual: BulletVisual,
    pub radius: f32,
    pub collision_radius: f32,
    pub lifetime_ticks: u32,
    pub flags: BulletFlags,
}
impl_stable_hash!(BulletType {
    visual,
    radius,
    collision_radius,
    lifetime_ticks,
    flags
});

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct BulletVisual {
    pub silhouette: u16,
    pub palette: u16,
    pub palette_space: u8,
    pub glow: u8,
}
impl_stable_hash!(BulletVisual {
    silhouette,
    palette,
    palette_space,
    glow
});

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct BulletFlags(pub u8);
impl StableHash for BulletFlags {
    fn stable_hash(&self, hasher: &mut StableHasher) {
        hasher.write_u8(self.0);
    }
}
impl BulletFlags {
    pub const SMASHABLE: Self = Self(1);
    pub const REFLECTABLE: Self = Self(2);
    pub const ENV_ACTIVE: Self = Self(4);
    pub const GRAZEABLE: Self = Self(8);
    pub fn contains(self, other: BulletFlags) -> bool {
        self.0 & other.0 == other.0
    }
}

#[derive(Debug)]
pub struct SigilLibrary {
    units: Vec<SigilUnit>,
    registry: Arc<BehaviorRegistry>,
    epoch: ContentEpoch,
}

impl SigilLibrary {
    pub fn new(units: Vec<SigilUnit>, registry: Arc<BehaviorRegistry>) -> Result<Self, SigilError> {
        let epoch = ContentEpoch::new(0, ContentManifestHash::EMPTY);
        Ok(Self {
            units,
            registry,
            epoch,
        })
    }
    pub fn units(&self) -> &[SigilUnit] {
        &self.units
    }
    pub fn unit(&self, id: UnitId) -> Option<&SigilUnit> {
        let _ = id;
        unimplemented!()
    }
    pub fn unit_index(&self, id: UnitId) -> Option<u16> {
        let _ = id;
        unimplemented!()
    }
    pub fn epoch(&self) -> ContentEpoch {
        self.epoch
    }
    pub fn registry_fingerprint(&self) -> u64 {
        self.registry.fingerprint()
    }
    pub fn registry(&self) -> &Arc<BehaviorRegistry> {
        &self.registry
    }
}

#[derive(Clone, Debug)]
pub struct SigilContent {
    library: Arc<SigilLibrary>,
}

impl StableHash for SigilContent {
    fn stable_hash(&self, hasher: &mut StableHasher) {
        self.library.epoch.stable_hash(hasher);
    }
}

impl SigilContent {
    pub fn library(&self) -> &SigilLibrary {
        &self.library
    }
    pub fn epoch(&self) -> ContentEpoch {
        self.library.epoch
    }
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SigilError {
    #[error(transparent)]
    Unit(#[from] UnitError),
    #[error("pool full")]
    PoolFull,
    #[error("duplicate unit {0}")]
    DuplicateUnit(UnitId),
    #[error("unknown unit {0}")]
    UnknownUnit(UnitId),
    #[error("too many units")]
    TooManyUnits,
    #[error("bullet type out of range")]
    BulletTypeOutOfRange { unit: UnitId, bullet_type: u16 },
    #[error("program out of range")]
    ProgramOutOfRange { unit: UnitId, program: u16 },
    #[error("cascade too deep")]
    CascadeTooDeep { depth: u8 },
    #[error("non-finite")]
    NonFinite,
    #[error("duplicate behavior {0:?}")]
    DuplicateBehavior(BehaviorId),
    #[error("unknown behavior")]
    UnknownBehavior { unit: UnitId, behavior: BehaviorId },
    #[error("registry mismatch")]
    RegistryMismatch { loaded: u64, given: u64 },
    #[error("already installed")]
    AlreadyInstalled,
    #[error("not installed")]
    NotInstalled,
    #[error("swap limit")]
    SwapLimit,
    #[error("content epoch mismatch: {snapshot:?} vs {loaded:?}")]
    ContentEpochMismatch {
        snapshot: Option<ContentEpoch>,
        loaded: Option<ContentEpoch>,
    },
}

// ---- §11.3 ------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct BulletId {
    index: u32,
    generation: u32,
}
impl fmt::Display for BulletId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}v{}", self.index, self.generation)
    }
}
impl StableHash for BulletId {
    fn stable_hash(&self, hasher: &mut StableHasher) {
        hasher.write_u64(self.to_bits());
    }
}
impl BulletId {
    pub fn index(self) -> u32 {
        self.index
    }
    pub fn generation(self) -> u32 {
        self.generation
    }
    pub fn to_bits(self) -> u64 {
        (u64::from(self.generation) << 32) | u64::from(self.index)
    }
    pub fn from_bits(bits: u64) -> Self {
        Self {
            index: bits as u32,
            generation: (bits >> 32) as u32,
        }
    }
}

#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BulletSpawn {
    pub unit: UnitId,
    pub bullet_type: u16,
    pub program: u16,
    pub position: Vec2,
    pub angle: f32,
    pub speed: f32,
    pub cascade: u8,
}

impl BulletSpawn {
    pub fn new(unit: UnitId, bullet_type: u16, position: Vec2, angle: f32, speed: f32) -> Self {
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
    #[must_use]
    pub fn with_program(mut self, program: u16) -> Self {
        self.program = program;
        self
    }
    #[must_use]
    pub fn with_cascade(mut self, cascade: u8) -> Self {
        self.cascade = cascade;
        self
    }
}

#[derive(Clone, Debug, Default)]
pub struct BulletPool {
    alive: Vec<bool>,
    generation: Vec<u32>,
    position: Vec<Vec2>,
    velocity: Vec<Vec2>,
    events: Vec<BulletEvent>,
    events_tick: u64,
    slot_count: u32,
}

impl StableHash for BulletPool {
    fn stable_hash(&self, hasher: &mut StableHasher) {
        hasher.write_u32(self.slot_count);
    }
}

impl BulletPool {
    pub const MAX_CAPACITY: u32 = 1 << 20;

    pub fn with_capacity(capacity: u32) -> Self {
        assert!(capacity <= Self::MAX_CAPACITY, "capacity above MAX_CAPACITY");
        Self::default()
    }
    pub fn capacity(&self) -> u32 {
        unimplemented!()
    }
    pub fn len(&self) -> usize {
        unimplemented!()
    }
    pub fn is_empty(&self) -> bool {
        unimplemented!()
    }
    pub fn slot_count(&self) -> u32 {
        self.slot_count
    }
    pub fn dropped_spawns(&self) -> u64 {
        unimplemented!()
    }
    pub fn is_alive(&self, id: BulletId) -> bool {
        let _ = id;
        unimplemented!()
    }
    pub fn get(&self, id: BulletId) -> Option<BulletRef<'_>> {
        let _ = id;
        unimplemented!()
    }
    pub fn iter(&self) -> impl Iterator<Item = BulletRef<'_>> {
        (0..self.slot_count as usize)
            .filter(|slot| self.alive[*slot])
            .map(|slot| BulletRef { pool: self, slot })
    }
    pub fn columns(&self) -> BulletColumns<'_> {
        unimplemented!()
    }
    pub fn blocks(&self) -> impl ExactSizeIterator<Item = PoolBlock<'_>> {
        slice_block_ranges(self.slot_count as usize).map(|slots| PoolBlock {
            index: slots.start / POOL_BLOCK_SIZE,
            slots,
            pool: self,
        })
    }
    pub fn spawn(&mut self, content: &SigilContent, spawn: BulletSpawn) -> Result<BulletId, SigilError> {
        let _ = (content, spawn);
        unimplemented!()
    }
    pub fn despawn(&mut self, id: BulletId, cause: DespawnCause) -> bool {
        let _ = (id, cause);
        unimplemented!()
    }
    pub fn clear(&mut self, content: &SigilContent, filter: &ClearFilter, cause: DespawnCause) -> u32 {
        let _ = (content, filter, cause);
        unimplemented!()
    }
    pub fn events(&self) -> &[BulletEvent] {
        &self.events
    }
    pub fn events_tick(&self) -> u64 {
        self.events_tick
    }
}

#[derive(Clone, Copy)]
pub struct BulletRef<'p> {
    pool: &'p BulletPool,
    slot: usize,
}

impl BulletRef<'_> {
    pub fn id(&self) -> BulletId {
        BulletId {
            index: self.slot as u32,
            generation: self.pool.generation[self.slot],
        }
    }
    pub fn unit_index(&self) -> u16 {
        unimplemented!()
    }
    pub fn bullet_type(&self) -> u16 {
        unimplemented!()
    }
    pub fn position(&self) -> Vec2 {
        self.pool.position[self.slot]
    }
}

/// Only produced by `BulletPool::columns` and `PoolBlock::columns` (§2 rule 13 exemption).
#[non_exhaustive]
pub struct BulletColumns<'p> {
    pub alive: &'p [bool],
    pub generation: &'p [u32],
    pub unit: &'p [u16],
    pub bullet_type: &'p [u16],
    pub program: &'p [u16],
    pub flags: &'p [BulletFlags],
    pub cascade: &'p [u8],
    pub position: &'p [Vec2],
    pub previous_position: &'p [Vec2],
    pub velocity: &'p [Vec2],
    pub angle: &'p [f32],
    pub speed: &'p [f32],
    pub age: &'p [u32],
    pub state: &'p [[f32; 4]],
}

pub struct PoolBlock<'p> {
    index: usize,
    slots: Range<usize>,
    pool: &'p BulletPool,
}

impl<'p> PoolBlock<'p> {
    pub fn index(&self) -> usize {
        self.index
    }
    pub fn slots(&self) -> Range<usize> {
        self.slots.clone()
    }
    pub fn columns(&self) -> BulletColumns<'p> {
        let _ = self.pool;
        unimplemented!()
    }
}

#[non_exhaustive]
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum DespawnCause {
    Lifetime,
    Bounds,
    Transform,
    Behavior,
    Clear,
    Swap,
    External,
}
impl StableHash for DespawnCause {
    fn stable_hash(&self, hasher: &mut StableHasher) {
        hasher.write_u8(*self as u8);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClearFilter {
    All,
    Unit(UnitId),
    Type(UnitId, u16),
    AnyFlags(BulletFlags),
}
impl StableHash for ClearFilter {
    fn stable_hash(&self, hasher: &mut StableHasher) {
        match self {
            ClearFilter::All => hasher.write_u8(0),
            ClearFilter::Unit(unit) => (1u8, *unit).stable_hash(hasher),
            ClearFilter::Type(unit, ty) => (2u8, *unit, *ty).stable_hash(hasher),
            ClearFilter::AnyFlags(flags) => (3u8, *flags).stable_hash(hasher),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BulletEvent {
    pub id: BulletId,
    pub unit: UnitId,
    pub bullet_type: u16,
    pub position: Vec2,
    pub cause: DespawnCause,
}
impl_stable_hash!(BulletEvent {
    id,
    unit,
    bullet_type,
    position,
    cause
});

// ---- §11.4 ------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct Emitter {
    pub unit: UnitId,
    pub emitter: u16,
    pub origin: Vec2,
    pub rotation: f32,
    pub started_at: u64,
}
impl_stable_hash!(Emitter {
    unit,
    emitter,
    origin,
    rotation,
    started_at
});

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AimTarget(pub Option<Vec2>);
impl StableHash for AimTarget {
    fn stable_hash(&self, hasher: &mut StableHasher) {
        self.0.stable_hash(hasher);
    }
}

#[derive(Clone, Debug)]
pub struct ClearRequest {
    pub filter: ClearFilter,
}
impl_stable_hash!(ClearRequest { filter });

// ---- §11.5 ------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct BehaviorId(pub u32);
impl StableHash for BehaviorId {
    fn stable_hash(&self, hasher: &mut StableHasher) {
        hasher.write_u32(self.0);
    }
}

#[derive(Clone, Copy, Debug)]
pub struct BehaviorInput<'a> {
    pub tick: u64,
    pub age: u32,
    pub params: &'a [f32],
    pub target: Option<Vec2>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BulletMotion {
    pub position: Vec2,
    pub velocity: Vec2,
    pub angle: f32,
    pub speed: f32,
    pub state: [f32; 4],
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BehaviorOutcome {
    Keep,
    Despawn,
}

pub type BehaviorFn = fn(&BehaviorInput<'_>, &mut BulletMotion, &mut SimRng) -> BehaviorOutcome;

pub struct BehaviorRegistryBuilder {
    version: u32,
    entries: BTreeMap<BehaviorId, (&'static str, BehaviorFn)>,
}

impl BehaviorRegistryBuilder {
    pub fn new(version: u32) -> Self {
        Self {
            version,
            entries: BTreeMap::new(),
        }
    }
    pub fn register(&mut self, id: BehaviorId, name: &'static str, f: BehaviorFn) -> Result<&mut Self, SigilError> {
        if self.entries.insert(id, (name, f)).is_some() {
            return Err(SigilError::DuplicateBehavior(id));
        }
        Ok(self)
    }
    pub fn build(self) -> Arc<BehaviorRegistry> {
        Arc::new(BehaviorRegistry {
            version: self.version,
            entries: self.entries,
        })
    }
}

#[derive(Debug)]
pub struct BehaviorRegistry {
    version: u32,
    entries: BTreeMap<BehaviorId, (&'static str, BehaviorFn)>,
}

impl BehaviorRegistry {
    pub fn version(&self) -> u32 {
        self.version
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn ids(&self) -> impl Iterator<Item = BehaviorId> + '_ {
        self.entries.keys().copied()
    }
    pub fn name(&self, id: BehaviorId) -> Option<&'static str> {
        self.entries.get(&id).map(|entry| entry.0)
    }
    pub fn get(&self, id: BehaviorId) -> Option<BehaviorFn> {
        self.entries.get(&id).map(|entry| entry.1)
    }
    pub fn fingerprint(&self) -> u64 {
        let mut hasher = StableHasher::new();
        hasher.write_u32(self.version);
        hasher.write_usize(self.entries.len());
        for (id, (name, _)) in &self.entries {
            hasher.write_u32(id.0);
            hasher.write_str(name);
        }
        hasher.finish()
    }
}

// ---- §11.6 ------------------------------------------------------------------------------------

#[non_exhaustive]
#[derive(Clone, Debug, PartialEq)]
pub struct SigilConfig {
    pub capacity: u32,
    pub bounds_min: Vec2,
    pub bounds_max: Vec2,
}

impl SigilConfig {
    pub fn new(capacity: u32, bounds_min: Vec2, bounds_max: Vec2) -> Self {
        Self {
            capacity,
            bounds_min,
            bounds_max,
        }
    }
}

pub fn install(
    sim: &mut Simulation,
    library: SigilLibrary,
    registry: Arc<BehaviorRegistry>,
    config: SigilConfig,
) -> Result<(), SigilError> {
    if sim.world().resource::<SigilContent>().is_some() {
        return Err(SigilError::AlreadyInstalled);
    }
    if library.registry_fingerprint() != registry.fingerprint() {
        return Err(SigilError::RegistryMismatch {
            loaded: library.registry_fingerprint(),
            given: registry.fingerprint(),
        });
    }
    let world = sim.world_mut();
    world.register_component::<Emitter>();
    world.register_component::<ClearRequest>();
    world.insert_resource(BulletPool::with_capacity(config.capacity));
    world.insert_resource(SigilContent {
        library: Arc::new(library),
    });
    if world.resource::<AimTarget>().is_none() {
        world.insert_resource(AimTarget::default());
    }
    let begin_registry = Arc::clone(&registry);
    let update_registry = registry;
    sim.schedule_mut()
        .add_system(system_fn(system_names::BEGIN, move |world: &mut World| {
            let expected = begin_registry.fingerprint();
            let loaded = world
                .resource::<SigilContent>()
                .map(|content| content.library().registry_fingerprint());
            assert_eq!(loaded, Some(expected), "behavior registry fingerprint does not match");
        }))
        .add_system(system_fn(system_names::UPDATE, move |world: &mut World| {
            sigil_update(world, &update_registry);
        }));
    Ok(())
}

pub mod system_names {
    pub const BEGIN: &str = "sigil.begin";
    pub const UPDATE: &str = "sigil.update";
    pub const RESOLVE: &str = "sigil.resolve";
    pub const EMIT: &str = "sigil.emit";
    pub const CLEAR: &str = "sigil.clear";
}

// ---- §11.7 ------------------------------------------------------------------------------------

pub const POOL_BLOCK_SIZE: usize = grimoire_ecs::QUERY_BLOCK_SIZE;

pub mod stream {
    use grimoire_sim_p1::stream::{engine_stream, owner};
    pub const EMIT: u64 = engine_stream(owner::SIGIL, 1);
    pub const UPDATE: u64 = engine_stream(owner::SIGIL, 2);
}

/// §11.7 flow: take the pool, split columns into blocks, run blocks while reading `&World`,
/// write the pool back.
fn sigil_update(world: &mut World, registry: &BehaviorRegistry) {
    let Some(pool) = world.resource_mut::<BulletPool>() else {
        return;
    };
    let mut pool = std::mem::take(pool);
    let slot_count = (pool.slot_count as usize)
        .min(pool.position.len())
        .min(pool.velocity.len());
    let mut positions: &mut [Vec2] = &mut pool.position[..slot_count];
    let mut velocities: &mut [Vec2] = &mut pool.velocity[..slot_count];
    let mut blocks = Vec::new();
    for range in slice_block_ranges(positions.len()) {
        let (p_head, p_tail) = std::mem::take(&mut positions).split_at_mut(range.len());
        let (v_head, v_tail) = std::mem::take(&mut velocities).split_at_mut(range.len());
        blocks.push((range.start / POOL_BLOCK_SIZE, p_head, v_head));
        positions = p_tail;
        velocities = v_tail;
    }
    let world_ref: &World = world;
    let seed = world_ref.resource::<SimSeed>().map_or(0, |seed| seed.0);
    let tick = world_ref.resource::<Tick>().map_or(0, |tick| tick.0);
    let despawns: Vec<Vec<usize>> = run_blocks(world_ref.executor(), blocks, |(index, p, v)| {
        let content = world_ref.resource::<SigilContent>();
        let target = world_ref.resource::<AimTarget>().and_then(|aim| aim.0);
        let mut rng = derive_block_rng(seed, tick, stream::UPDATE, index as u64);
        let mut out = Vec::new();
        for (slot, (position, velocity)) in p.iter_mut().zip(v.iter_mut()).enumerate() {
            let mut motion = BulletMotion {
                position: *position,
                velocity: *velocity,
                angle: 0.0,
                speed: 0.0,
                state: [0.0; 4],
            };
            let input = BehaviorInput {
                tick,
                age: 0,
                params: &[],
                target,
            };
            if let Some(behavior) = registry.get(BehaviorId(0))
                && behavior(&input, &mut motion, &mut rng) == BehaviorOutcome::Despawn
            {
                out.push(slot);
            }
            *position = motion.position + motion.velocity;
            let _ = content;
        }
        out
    });
    let _ = despawns;
    if let Some(slot) = world.resource_mut::<BulletPool>() {
        *slot = pool;
    }
}

/// §11.4 emitter scatter stream.
pub fn emitter_rng(world: &World, entity: Entity) -> SimRng {
    let seed = world.resource::<SimSeed>().map_or(0, |seed| seed.0);
    let tick = world.resource::<Tick>().map_or(0, |tick| tick.0);
    derive_block_rng(seed, tick, stream::EMIT, entity.to_bits())
}

// ---- §11.8 ------------------------------------------------------------------------------------

#[non_exhaustive]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ContentEpoch {
    pub swaps: u32,
    pub manifest_hash: ContentManifestHash,
}
impl_stable_hash!(ContentEpoch {
    swaps,
    manifest_hash
});

impl ContentEpoch {
    pub fn new(swaps: u32, manifest_hash: ContentManifestHash) -> Self {
        Self {
            swaps,
            manifest_hash,
        }
    }
}

#[non_exhaustive]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SwapReport {
    pub unit: UnitId,
    pub epoch: ContentEpoch,
    pub effective_tick: u64,
    pub restarted_emitters: u32,
    pub despawned_bullets: u32,
}

pub fn replace_unit(sim: &mut Simulation, unit: SigilUnit) -> Result<SwapReport, SigilError> {
    let _ = (sim, unit);
    unimplemented!()
}

pub fn restore_checked(sim: &mut Simulation, snapshot: &SimSnapshot) -> Result<(), SigilError> {
    let loaded = sim.world().resource::<SigilContent>().map(SigilContent::epoch);
    sim.restore_checked(snapshot, |snapshot| {
        let in_snapshot = snapshot.resource::<SigilContent>().map(SigilContent::epoch);
        if in_snapshot == loaded {
            Ok(())
        } else {
            Err(SigilError::ContentEpochMismatch {
                snapshot: in_snapshot,
                loaded,
            })
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_resource<R: grimoire_ecs::Resource>() {}
    fn assert_component<C: grimoire_ecs::Component>() {}
    fn assert_send_sync<T: Send + Sync>() {}

    fn keep(_: &BehaviorInput<'_>, _: &mut BulletMotion, _: &mut SimRng) -> BehaviorOutcome {
        BehaviorOutcome::Keep
    }

    #[test]
    fn bounds() {
        assert_resource::<BulletPool>();
        assert_resource::<SigilContent>();
        assert_resource::<AimTarget>();
        assert_component::<Emitter>();
        assert_component::<ClearRequest>();
        assert_send_sync::<SigilLibrary>();
        assert_send_sync::<BehaviorRegistry>();
        assert_send_sync::<World>();
        let mut builder = BehaviorRegistryBuilder::new(1);
        builder
            .register(BehaviorId(1), "keep", keep)
            .expect("register")
            .register(BehaviorId(2), "keep2", keep)
            .expect("register");
        let registry = builder.build();
        assert_eq!(registry.len(), 2);
        assert_ne!(stream::EMIT, stream::UPDATE);
    }
}
