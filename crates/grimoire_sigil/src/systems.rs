//! Installation and the five `sigil.*` tick-phase systems (contract §11.6/§11.7, PRD-0004).
//!
//! The seven blocks (`crate::blocks`) and six stackable modifiers (`crate::runtime`) run here,
//! end to end, from [`install`] through all five phases (WP5.1). Since WP5.2 `sigil.update` also
//! raises the tick's [`EventRequest`]s, calls each bullet type's bound `BulletBehavior` and runs
//! its transforms, and [`ResolveSystem`] folds the resulting sub-spawns after the despawns.

use std::sync::Arc;

use grimoire_core::{StableHash, StableHasher, Vec2};
use grimoire_ecs::{Entity, System, World, run_blocks};
use grimoire_sim::{SimSeed, Simulation, Tick, derive_block_rng};

use crate::behavior::BehaviorRegistry;
use crate::blocks::{self, Shot};
use crate::content::{SigilContent, SigilLibrary};
use crate::emitter::{AimTarget, ClearFilter, ClearRequest, Emitter, EventRequest};
use crate::error::SigilError;
use crate::pool::{BulletPool, BulletSpawn, DespawnCause, PendingDespawn, PendingSpawn};
use crate::runtime::{self, BlockOutcome, RuntimeCache, UpdateContext};
use crate::unit::{EmitterRecord, SigilUnit};

/// Names of the five stages [`install`] appends, in schedule order (contract §11.6). Appear in
/// [`grimoire_ecs::Schedule::stages`]/`system_names` and in every `SystemObserver` callback.
pub mod system_names {
    /// Resets despawn events, checks the behavior-registry fingerprint (contract §8.4).
    pub const BEGIN: &str = "sigil.begin";
    /// Raises the tick's events, then advances every live bullet: block motion, the modifier
    /// stack, the bullet type's behavior and transforms, then the lifetime/bounds check.
    pub const UPDATE: &str = "sigil.update";
    /// Applies despawns, then sub-spawns, collected by `UPDATE`, in block order.
    pub const RESOLVE: &str = "sigil.resolve";
    /// Fires due emitters.
    pub const EMIT: &str = "sigil.emit";
    /// Applies [`crate::ClearRequest`]s.
    pub const CLEAR: &str = "sigil.clear";
}

/// Data-parallel block size for `BulletPool::update_blocks_mut` (contract §11.7): the same
/// constant `grimoire_ecs` itself splits queries at, so a pool and a query block never disagree
/// about where one block ends.
pub const POOL_BLOCK_SIZE: usize = grimoire_ecs::QUERY_BLOCK_SIZE;

/// Random-stream numbers this crate draws from (contract §8.3/§11.7).
pub mod stream {
    use grimoire_sim::stream::{engine_stream, owner};

    /// `sigil.emit`'s stream, keyed by `Entity::to_bits()` (contract §11.4/§11.7): `scatter`'s
    /// jitter, and any future emitter-level randomness.
    pub const EMIT: u64 = engine_stream(owner::SIGIL, 1);
    /// `sigil.update`'s stream, keyed by pool block index (contract §11.7): drawn from, per block
    /// in slot order, by `BulletBehavior`s (contract §11.5) and by the `scatter` blocks of
    /// `burst`/`become_emitter` volleys. The six modifiers never draw from it.
    pub const UPDATE: u64 = engine_stream(owner::SIGIL, 2);
}

/// Configuration [`install`] needs beyond the library and registry (contract §11.6).
///
/// `#[non_exhaustive]`: constructed through [`SigilConfig::new`], so a later field can be added
/// without breaking callers.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct SigilConfig {
    /// [`BulletPool`] capacity, allocated in full by `install` (contract §11.3).
    pub capacity: u32,
    /// Lower simulation bound; a bullet whose position leaves `bounds_min..=bounds_max` on either
    /// axis is despawned with [`DespawnCause::Bounds`] (`sigil.update`).
    pub bounds_min: Vec2,
    /// Upper simulation bound.
    pub bounds_max: Vec2,
}

impl SigilConfig {
    /// Builds a configuration from its fields.
    #[must_use]
    pub const fn new(capacity: u32, bounds_min: Vec2, bounds_max: Vec2) -> Self {
        Self {
            capacity,
            bounds_min,
            bounds_max,
        }
    }
}

/// Despawns and sub-spawns [`UpdateSystem`] decided but could not apply itself (its blocks only
/// ever own a disjoint sub-slice of the pool, contract §11.7), staged here for
/// [`ResolveSystem`]. Both lists are working buffers: always empty except momentarily between
/// those two stages of one tick, so they never carry state across a tick boundary. Not part of the
/// public contract, so this stays `pub(crate)`.
#[derive(Debug, Clone, Default)]
pub(crate) struct PendingOutcomes {
    despawns: Vec<PendingDespawn>,
    spawns: Vec<PendingSpawn>,
}

impl StableHash for PendingOutcomes {
    /// Feeds the despawn list exactly as the WP5.1 resource did (its registration slot and layout
    /// are part of every installed session's `World::stable_hash`). The sub-spawn list is not fed:
    /// it is empty whenever a hash can be taken, and feeding it would change that layout.
    fn stable_hash(&self, hasher: &mut StableHasher) {
        hasher.write_usize(self.despawns.len());
        for despawn in &self.despawns {
            hasher.write_u32(despawn.index);
            despawn.cause.stable_hash(hasher);
        }
    }
}

/// Installs Sigil content and the five `sigil.*` systems into `sim` (contract §11.6).
///
/// Registers [`Emitter`]/[`ClearRequest`], then inserts [`BulletPool::with_capacity`],
/// [`SigilContent`] (fresh epoch) and, if not already present, a default [`AimTarget`] — in that
/// order, since it is part of [`grimoire_ecs::World::stable_hash`]'s resource layout — before
/// appending the five systems to the current end of `sim`'s schedule.
///
/// # Errors
///
/// - [`SigilError::AlreadyInstalled`] if [`SigilContent`] is already present.
/// - [`SigilError::RegistryMismatch`] if `library.registry_fingerprint() !=
///   registry.fingerprint()`.
///
/// Neither error changes `sim`.
pub fn install(
    sim: &mut Simulation,
    library: SigilLibrary,
    registry: Arc<BehaviorRegistry>,
    config: SigilConfig,
) -> Result<(), SigilError> {
    if sim.world().resource::<SigilContent>().is_some() {
        return Err(SigilError::AlreadyInstalled);
    }
    let loaded = library.registry_fingerprint();
    let given = registry.fingerprint();
    if loaded != given {
        return Err(SigilError::RegistryMismatch { loaded, given });
    }

    let world = sim.world_mut();
    world.register_component::<Emitter>();
    world.register_component::<ClearRequest>();

    let mut pool = BulletPool::with_capacity(config.capacity);
    pool.set_bounds(config.bounds_min, config.bounds_max);
    world.insert_resource(pool);
    world.insert_resource(SigilContent::new(library));
    if world.resource::<AimTarget>().is_none() {
        world.insert_resource(AimTarget::default());
    }
    world.insert_resource(PendingOutcomes::default());

    sim.schedule_mut()
        .add_system(BeginSystem {
            fingerprint: registry.fingerprint(),
        })
        .add_system(UpdateSystem::default())
        .add_system(ResolveSystem)
        .add_system(EmitSystem::default())
        .add_system(ClearSystem::default());
    Ok(())
}

/// Puts `pool` back into the world's existing [`BulletPool`] slot, which `std::mem::take` left
/// holding an empty default pool (contract §11.7). Writing into the slot instead of calling
/// `World::insert_resource` keeps the resource's position and avoids boxing it anew every tick
/// (WP5.4: no allocation per spawn, despawn or tick in the pool path).
fn put_pool_back(world: &mut World, pool: BulletPool) {
    match world.resource_mut::<BulletPool>() {
        Some(slot) => *slot = pool,
        None => world.insert_resource(pool),
    }
}

/// `sigil.begin` (contract §11.6): resets despawn events for the new tick and checks the loaded
/// content's behavior-registry fingerprint against the one `install` was given.
struct BeginSystem {
    /// Fingerprint of the registry `install` was given; the registry is immutable (contract
    /// §11.5), so it is computed once instead of every tick.
    fingerprint: u64,
}

impl System for BeginSystem {
    fn name(&self) -> &str {
        system_names::BEGIN
    }

    fn run(&mut self, world: &mut World) {
        let tick = world.resource::<Tick>().copied().unwrap_or_default().0;
        if let Some(content) = world.resource::<SigilContent>() {
            let given = self.fingerprint;
            let loaded = content.library().registry_fingerprint();
            assert!(
                given == loaded,
                "behavior registry fingerprint {given:#018x} does not match the loaded content \
                 ({loaded:#018x})"
            );
        }
        if let Some(pool) = world.resource_mut::<BulletPool>() {
            pool.begin_tick(tick);
        }
    }
}

/// `sigil.update` (contract §11.6/§11.7): raises the tick's [`EventRequest`]s, then advances
/// every live bullet through [`runtime::update_block`], one data-parallel block at a time through
/// the world's executor.
///
/// Every field besides the cache is a working buffer (WP5.4): filled and consumed within one run,
/// cleared but never shrunk, so a steady tick allocates nothing for events, despawns or
/// sub-spawns, and none of it is state.
#[derive(Default)]
struct UpdateSystem {
    cache: RuntimeCache,
    /// The event ids raised this tick, ascending and deduplicated.
    events: Vec<u32>,
    /// The live `EventRequest`s of this tick, in query order.
    requests: Vec<(Entity, u32)>,
    /// One outcome per pool block, reused across ticks.
    outcomes: Vec<BlockOutcome>,
}

impl System for UpdateSystem {
    fn name(&self) -> &str {
        system_names::UPDATE
    }

    fn run(&mut self, world: &mut World) {
        let Some(content) = world.resource::<SigilContent>().cloned() else {
            return;
        };
        self.cache.ensure(content.library());
        let Some(bounds) = world.resource::<BulletPool>().map(BulletPool::bounds) else {
            return;
        };
        let (bounds_min, bounds_max) = bounds;

        // Contract §11.9: every live request raises its event for this tick, in query order, and
        // its entity is despawned in that order. An unregistered `EventRequest` (no game ever
        // spawned one) simply matches nothing.
        self.events.clear();
        self.requests.clear();
        self.requests.extend(
            world
                .query::<(Entity, &EventRequest)>()
                .map(|(entity, request)| (entity, request.event.0)),
        );
        for &(entity, event) in &self.requests {
            self.events.push(event);
            world.despawn(entity);
        }
        self.events.sort_unstable();
        self.events.dedup();

        let tick = world.resource::<Tick>().copied().unwrap_or_default().0;
        let seed = world.resource::<SimSeed>().copied().unwrap_or_default().0;
        let aim = world.resource::<AimTarget>().copied().unwrap_or_default().0;
        let mut pool = world
            .resource_mut::<BulletPool>()
            .map(std::mem::take)
            .unwrap_or_default();

        let blocks = pool.update_blocks_mut();
        let block_count = blocks.len();
        if self.outcomes.len() < block_count {
            self.outcomes
                .resize_with(block_count, BlockOutcome::default);
        }
        let ctx = UpdateContext {
            library: content.library(),
            cache: &self.cache,
            bounds_min,
            bounds_max,
            tick,
            seed,
            aim,
            events: &self.events,
        };
        let work: Vec<_> = blocks.into_iter().zip(self.outcomes.iter_mut()).collect();
        run_blocks(world.executor(), work, |(mut block, outcome)| {
            runtime::update_block(&mut block, &ctx, outcome);
        });

        put_pool_back(world, pool);
        if let Some(pending) = world.resource_mut::<PendingOutcomes>() {
            for outcome in &self.outcomes[..block_count] {
                pending.despawns.extend_from_slice(&outcome.despawns);
                pending.spawns.extend_from_slice(&outcome.spawns);
            }
        }
    }
}

/// `sigil.resolve` (contract §11.6): applies every despawn `sigil.update`'s blocks collected, then
/// every sub-spawn, each in block order (the order [`PendingOutcomes`] were appended in). A
/// sub-spawn is one cascade level deeper than its parent; one that would exceed
/// [`SigilUnit::MAX_CASCADE_DEPTH`], or finds the pool full, is dropped and counted in
/// [`BulletPool::dropped_spawns`].
struct ResolveSystem;

impl System for ResolveSystem {
    fn name(&self) -> &str {
        system_names::RESOLVE
    }

    fn run(&mut self, world: &mut World) {
        let Some(slot) = world.resource_mut::<PendingOutcomes>() else {
            return;
        };
        if slot.despawns.is_empty() && slot.spawns.is_empty() {
            return;
        }
        // Taken out and written back into the same slot, so both lists keep their capacity.
        let mut pending = std::mem::take(slot);
        let content = world.resource::<SigilContent>().cloned();
        if let Some(pool) = world.resource_mut::<BulletPool>() {
            for despawn in &pending.despawns {
                pool.despawn_pending(despawn.index, despawn.cause);
            }
            if let Some(content) = &content {
                for &spawn in &pending.spawns {
                    apply_sub_spawn(pool, content, spawn);
                }
            }
        }
        pending.despawns.clear();
        pending.spawns.clear();
        if let Some(slot) = world.resource_mut::<PendingOutcomes>() {
            *slot = pending;
        }
    }
}

/// Spawns one sub-bullet for `sigil.resolve` (contract §11.6): the depth is the parent's plus one,
/// computed without overflowing; too deep or a full pool drops the spawn deterministically.
fn apply_sub_spawn(pool: &mut BulletPool, content: &SigilContent, spawn: PendingSpawn) {
    let Some(cascade) = spawn
        .parent_cascade
        .checked_add(1)
        .filter(|&depth| depth <= SigilUnit::MAX_CASCADE_DEPTH)
    else {
        pool.record_dropped_spawn();
        return;
    };
    let Some(unit) = content.library().units().get(usize::from(spawn.unit_index)) else {
        return; // unreachable: the parent bullet's unit index was valid this tick.
    };
    let request = BulletSpawn::new(
        unit.id(),
        spawn.bullet_type,
        spawn.position,
        spawn.angle,
        spawn.speed,
    )
    .with_program(spawn.program)
    .with_cascade(cascade);
    // As in `fire_emitter`: only a full pool is a runtime condition; every other error is
    // excluded by the decoder's range checks of the unit the sub-spawn comes from.
    if let Err(SigilError::PoolFull) = pool.spawn(content, request) {
        pool.record_dropped_spawn();
    }
}

/// `sigil.emit` (contract §11.4/§11.6): fires every due [`Emitter`], in query order.
///
/// Both fields are working buffers (WP5.4), cleared but never shrunk.
#[derive(Default)]
struct EmitSystem {
    emitters: Vec<(Entity, Emitter)>,
    shots: Vec<Shot>,
}

impl System for EmitSystem {
    fn name(&self) -> &str {
        system_names::EMIT
    }

    fn run(&mut self, world: &mut World) {
        let Some(content) = world.resource::<SigilContent>().cloned() else {
            return;
        };
        let tick = world.resource::<Tick>().copied().unwrap_or_default().0;
        let seed = world.resource::<SimSeed>().copied().unwrap_or_default().0;
        let aim = world.resource::<AimTarget>().copied().unwrap_or_default().0;

        self.emitters.clear();
        self.emitters.extend(
            world
                .query::<(Entity, &Emitter)>()
                .map(|(entity, emitter)| (entity, emitter.clone())),
        );
        if self.emitters.is_empty() {
            return;
        }

        let Some(mut pool) = world.resource_mut::<BulletPool>().map(std::mem::take) else {
            return;
        };
        let library = content.library();

        for (entity, emitter) in &self.emitters {
            fire_emitter(
                &mut pool,
                &content,
                library,
                emitter,
                *entity,
                tick,
                seed,
                aim,
                &mut self.shots,
            );
        }

        put_pool_back(world, pool);
    }
}

/// Fires `emitter` once if it is due at `tick` (contract §11.4: a pure function of unit, emitter
/// index, local time, origin, rotation, `AimTarget`, seed, tick and entity). A no-op, without
/// drawing any randomness, for an inactive emitter (unknown unit, out-of-range emitter index, or
/// `tick < started_at`).
#[allow(clippy::too_many_arguments)]
fn fire_emitter(
    pool: &mut BulletPool,
    content: &SigilContent,
    library: &SigilLibrary,
    emitter: &Emitter,
    entity: Entity,
    tick: u64,
    seed: u64,
    aim: Option<Vec2>,
    shots: &mut Vec<Shot>,
) {
    let Some(unit_index) = library.unit_index(emitter.unit) else {
        return;
    };
    let sigil_unit = &library.units()[unit_index as usize];
    if emitter.emitter >= sigil_unit.emitter_count() {
        return;
    }
    if tick < emitter.started_at {
        return;
    }
    let local_time = tick - emitter.started_at;
    let record = &sigil_unit.emitters()[emitter.emitter as usize];

    let delay = u64::from(record.delay_ticks);
    if local_time < delay {
        return;
    }
    let since_delay = local_time - delay;
    let interval = u64::from(record.interval_ticks);
    let (fires, volley_index, fire_number) = if interval == 0 {
        (since_delay == 0, 0u32, 1u32)
    } else if since_delay.is_multiple_of(interval) {
        let volley = (since_delay / interval) as u32;
        (true, volley, volley + 1)
    } else {
        (false, 0, 0)
    };
    if !fires {
        return;
    }
    if record.repeat != EmitterRecord::FOREVER && fire_number > record.repeat {
        return;
    }

    let origin = emitter.origin + Vec2::new(record.offset_x, record.offset_y);
    let base_angle = emitter.rotation;
    let base_speed = record.speed;

    // Contract §11.3/`BulletPool::spawn`'s doc comment: `BulletSpawn.program` is one-based (`0` =
    // no program), unlike this wire field's own `0`-based-or-`0xFFFF` convention — shifted here so
    // `record.program == 0` (a real reference to `programs()[0]`) is never confused with "none".
    let pool_program = if record.program == EmitterRecord::NO_PROGRAM {
        0
    } else {
        record.program + 1
    };
    let resolved = runtime::resolve_program(sigil_unit, pool_program);

    match resolved {
        Some((program, _)) => {
            let block_key = entity.to_bits() ^ u64::from(program.block.seed_hash);
            let mut rng = derive_block_rng(seed, tick, stream::EMIT, block_key);
            blocks::program_shots_into(
                program,
                base_speed,
                base_angle,
                aim,
                origin,
                volley_index,
                &mut rng,
                shots,
            );
        }
        // No program at all (`record.program == NO_PROGRAM`, or — unreachable for a decoded
        // unit, since the decoder already range-checks `EmitterRecord::program` — a stale
        // reference): the bullet type's plain spawn, no block/modifier stack.
        None => {
            shots.clear();
            shots.push(Shot {
                angle: base_angle,
                speed: base_speed,
                offset: Vec2::ZERO,
            });
        }
    }

    for shot in shots.iter() {
        let spawn = BulletSpawn::new(
            emitter.unit,
            record.bullet_type,
            origin + shot.offset,
            shot.angle,
            shot.speed,
        )
        .with_program(pool_program);
        // Contract §11.3: the runtime discards a spawn a caller has no `Result` channel for and
        // counts it in `dropped_spawns`, deterministically, never a panic. Every other error
        // (`UnknownUnit`, `BulletTypeOutOfRange`, ...) cannot happen here — `record.bullet_type`
        // and `pool_program` were already validated when this unit was decoded/loaded — so it is
        // silently ignored rather than treated as unreachable, per contract §2 rule 9.
        match pool.spawn(content, spawn) {
            Ok(_) => {}
            Err(SigilError::PoolFull) => pool.record_dropped_spawn(),
            Err(_) => {}
        }
    }
}

/// `sigil.clear` (contract §11.4/§11.6): applies every live [`ClearRequest`], in query order, then
/// despawns the requesting entities.
#[derive(Default)]
struct ClearSystem {
    /// Working buffer: this tick's requests, in query order.
    requests: Vec<(Entity, ClearFilter)>,
}

impl System for ClearSystem {
    fn name(&self) -> &str {
        system_names::CLEAR
    }

    fn run(&mut self, world: &mut World) {
        let Some(content) = world.resource::<SigilContent>().cloned() else {
            return;
        };
        self.requests.clear();
        self.requests.extend(
            world
                .query::<(Entity, &ClearRequest)>()
                .map(|(entity, request)| (entity, request.filter.clone())),
        );
        if self.requests.is_empty() {
            return;
        }
        if let Some(mut pool) = world.resource_mut::<BulletPool>().map(std::mem::take) {
            for (_, filter) in &self.requests {
                pool.clear(&content, filter, DespawnCause::Clear);
            }
            put_pool_back(world, pool);
        }
        for &(entity, _) in &self.requests {
            world.despawn(entity);
        }
    }
}
