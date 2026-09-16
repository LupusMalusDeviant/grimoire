//! Installation and the five `sigil.*` tick-phase systems (contract §11.6/§11.7, PRD-0004).
//!
//! **Scope of WP5.1** (see the crate's WP5.1 pull request for the full list): the seven blocks
//! (`crate::blocks`) and six stackable modifiers (`crate::runtime`) run here, end to end, from
//! [`install`] through all five phases. Sub-spawns and per-bullet-type transforms
//! (`Transforms`/§10.4's `change_type`/`become_emitter`/`burst`/`reverse`) are not: nothing in the
//! wire format yet ties a bullet type to either a transform program or a behavior
//! (`docs/formats/sigil.md` §11.4, open points 1 and 4), so [`ResolveSystem`] only ever folds
//! despawns in this work package — the sub-spawn fold contract §11.6 describes stays a documented
//! hook for WP5.2, which is also where a `BehaviorFn` first gets called from a tick phase.

use std::sync::Arc;

use grimoire_core::{StableHash, StableHasher, Vec2};
use grimoire_ecs::{Entity, System, World, run_blocks};
use grimoire_sim::{SimSeed, Simulation, Tick, derive_block_rng};

use crate::behavior::BehaviorRegistry;
use crate::blocks::{self, Shot};
use crate::content::{SigilContent, SigilLibrary};
use crate::emitter::{AimTarget, ClearFilter, ClearRequest, Emitter};
use crate::error::SigilError;
use crate::pool::{BulletPool, BulletSpawn, DespawnCause, PendingDespawn};
use crate::runtime::{self, RuntimeCache};
use crate::unit::{EmitterRecord, ModifierKind};

/// Names of the five stages [`install`] appends, in schedule order (contract §11.6). Appear in
/// [`grimoire_ecs::Schedule::stages`]/`system_names` and in every `SystemObserver` callback.
pub mod system_names {
    /// Resets despawn events, checks the behavior-registry fingerprint (contract §8.4).
    pub const BEGIN: &str = "sigil.begin";
    /// Advances every live bullet: block motion, the modifier stack, then the bounds/lifetime
    /// check.
    pub const UPDATE: &str = "sigil.update";
    /// Applies despawns collected by `UPDATE`, in block order.
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
    /// `sigil.update`'s stream, keyed by pool block index (contract §11.7). Unused by WP5.1's six
    /// modifiers (all deterministic given a bullet's own state) but reserved: a future
    /// `BulletBehavior` (contract §11.5) draws from exactly this stream, per block, in slot order.
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

/// Despawns [`crate::systems::UpdateSystem`] decided but could not apply itself (its blocks only
/// ever own a disjoint sub-slice of the pool, contract §11.7), staged here for
/// [`ResolveSystem`]. Always empty except momentarily between those two stages of one tick; not
/// part of the public contract, so this stays `pub(crate)`.
#[derive(Debug, Clone, Default)]
pub(crate) struct PendingDespawns(Vec<PendingDespawn>);

impl StableHash for PendingDespawns {
    fn stable_hash(&self, hasher: &mut StableHasher) {
        hasher.write_usize(self.0.len());
        for despawn in &self.0 {
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
    world.insert_resource(PendingDespawns::default());

    sim.schedule_mut()
        .add_system(BeginSystem { registry })
        .add_system(UpdateSystem {
            cache: RuntimeCache::default(),
        })
        .add_system(ResolveSystem)
        .add_system(EmitSystem)
        .add_system(ClearSystem);
    Ok(())
}

/// `sigil.begin` (contract §11.6): resets despawn events for the new tick and checks the loaded
/// content's behavior-registry fingerprint against the one `install` was given.
struct BeginSystem {
    registry: Arc<BehaviorRegistry>,
}

impl System for BeginSystem {
    fn name(&self) -> &str {
        system_names::BEGIN
    }

    fn run(&mut self, world: &mut World) {
        let tick = world.resource::<Tick>().copied().unwrap_or_default().0;
        if let Some(content) = world.resource::<SigilContent>() {
            let given = self.registry.fingerprint();
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

/// `sigil.update` (contract §11.6/§11.7): advances every live bullet through
/// [`runtime::update_block`], one data-parallel block at a time through the world's executor.
struct UpdateSystem {
    cache: RuntimeCache,
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
        let mut pool = world
            .resource_mut::<BulletPool>()
            .map(std::mem::take)
            .unwrap_or_default();

        let blocks = pool.update_blocks_mut();
        let library = content.library();
        let cache = &self.cache;
        let results: Vec<Vec<PendingDespawn>> =
            run_blocks(world.executor(), blocks, |mut block| {
                runtime::update_block(&mut block, library, cache, bounds_min, bounds_max)
            });

        world.insert_resource(pool);
        let mut pending = world
            .resource_mut::<PendingDespawns>()
            .map(std::mem::take)
            .unwrap_or_default();
        for mut block_despawns in results {
            pending.0.append(&mut block_despawns);
        }
        world.insert_resource(pending);
    }
}

/// `sigil.resolve` (contract §11.6): applies every despawn `sigil.update`'s blocks collected, in
/// block order (the order [`PendingDespawns`] were appended in). Sub-spawn folding is WP5.2 scope
/// (module docs).
struct ResolveSystem;

impl System for ResolveSystem {
    fn name(&self) -> &str {
        system_names::RESOLVE
    }

    fn run(&mut self, world: &mut World) {
        let Some(mut pending) = world.resource_mut::<PendingDespawns>().map(std::mem::take) else {
            return;
        };
        if !pending.0.is_empty()
            && let Some(pool) = world.resource_mut::<BulletPool>()
        {
            for despawn in pending.0.drain(..) {
                pool.despawn_pending(despawn.index, despawn.cause);
            }
        }
        world.insert_resource(pending);
    }
}

/// `sigil.emit` (contract §11.4/§11.6): fires every due [`Emitter`], in query order.
struct EmitSystem;

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

        let emitters: Vec<(Entity, Emitter)> = world
            .query::<(Entity, &Emitter)>()
            .map(|(entity, emitter)| (entity, emitter.clone()))
            .collect();
        if emitters.is_empty() {
            return;
        }

        let Some(mut pool) = world.resource_mut::<BulletPool>().map(std::mem::take) else {
            return;
        };
        let library = content.library();

        for (entity, emitter) in emitters {
            fire_emitter(
                &mut pool, &content, library, &emitter, entity, tick, seed, aim,
            );
        }

        world.insert_resource(pool);
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

    let shots: Vec<Shot> = match resolved {
        Some((program, _)) => {
            let block_key = entity.to_bits() ^ u64::from(program.block.seed_hash);
            let mut rng = derive_block_rng(seed, tick, stream::EMIT, block_key);
            let mut shots = blocks::generate_shots(
                &program.block,
                base_speed,
                base_angle,
                aim,
                origin,
                volley_index,
                &mut rng,
            );
            for modifier in &program.modifiers {
                if modifier.kind == ModifierKind::MIRROR {
                    shots = blocks::apply_mirror(&shots, modifier, base_angle);
                }
            }
            shots
        }
        // No program at all (`record.program == NO_PROGRAM`, or — unreachable for a decoded
        // unit, since the decoder already range-checks `EmitterRecord::program` — a stale
        // reference): the bullet type's plain spawn, no block/modifier stack.
        None => vec![Shot {
            angle: base_angle,
            speed: base_speed,
            offset: Vec2::ZERO,
        }],
    };

    for shot in shots {
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
struct ClearSystem;

impl System for ClearSystem {
    fn name(&self) -> &str {
        system_names::CLEAR
    }

    fn run(&mut self, world: &mut World) {
        let Some(content) = world.resource::<SigilContent>().cloned() else {
            return;
        };
        let requests: Vec<(Entity, ClearFilter)> = world
            .query::<(Entity, &ClearRequest)>()
            .map(|(entity, request)| (entity, request.filter.clone()))
            .collect();
        if requests.is_empty() {
            return;
        }
        if let Some(mut pool) = world.resource_mut::<BulletPool>().map(std::mem::take) {
            for (_, filter) in &requests {
                pool.clear(&content, filter, DespawnCause::Clear);
            }
            world.insert_resource(pool);
        }
        for (entity, _) in requests {
            world.despawn(entity);
        }
    }
}
