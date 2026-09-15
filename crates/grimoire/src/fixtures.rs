//! Minimal, game-neutral player proxy (contract §9.5, Cargo feature `fixtures`).
//!
//! The camera's follow target, the anchor of `Aimed`-style patterns, the centre of the graze ring
//! and the steering target of the "Idle" and "Random Dodger" bot profiles (Plan 0002 WP7.4) are
//! all, until a game supplies its own player, one and the same entity. [`PlayerProxyPlugin`] is
//! that entity: the only real feature this work package (WP1) ships (Plan 0002 WP1.3); a game
//! replaces it with its own systems from P2 onward, but the focus hook and `sample_aim` (§9.4)
//! stay usable without this feature.
//!
//! Deliberately out of scope here (later work packages): `GamePlugin::focus`, `extract_stage` and
//! the rest of the §9.2 main-loop wiring that will read [`ProxyPosition`]/[`ProxyPreviousPosition`]
//! for camera following (WP2.4) — until then, a consumer reads them directly through
//! [`proxy_entity`]. Bot profiles that steer this proxy through synthesized input land in WP7.4.

use grimoire_collide::{Aabb, GrazeRing, LayerMask};
use grimoire_core::math::dmath;
use grimoire_core::{StableHash, StableHasher, Vec2, impl_stable_hash};
use grimoire_ecs::{Access, Entity, With, World, parallel_system_fn};
use grimoire_sigil::AimTarget;
use grimoire_sim::{MAX_INPUT_SLOTS, Simulation, TickInput};

use crate::adapters::sigil_collide::GrazeProbe;
use crate::plugin::GamePlugin;

/// Component marker: the one entity a [`PlayerProxyPlugin`] spawns per simulation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PlayerProxy;
impl_stable_hash!(PlayerProxy {});

/// Component: position of the proxy after the most recently run tick.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ProxyPosition(pub Vec2);

impl StableHash for ProxyPosition {
    fn stable_hash(&self, hasher: &mut StableHasher) {
        self.0.stable_hash(hasher);
    }
}

/// Component: position of the proxy before the most recently run tick, the start point of render
/// interpolation.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ProxyPreviousPosition(pub Vec2);

impl StableHash for ProxyPreviousPosition {
    fn stable_hash(&self, hasher: &mut StableHasher) {
        self.0.stable_hash(hasher);
    }
}

/// Component: input axes 2/3 of the most recently run tick, as read from the raw [`TickInput`]
/// slot (not the derived [`AimTarget`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProxyAim(pub [i16; 2]);

impl StableHash for ProxyAim {
    fn stable_hash(&self, hasher: &mut StableHasher) {
        self.0.stable_hash(hasher);
    }
}

/// Resource: configuration of one [`PlayerProxyPlugin`] (contract §9.5).
///
/// `#[non_exhaustive]`: adjust by field assignment onto [`ProxyConfig::default`] (contract §2 rule
/// 13), so a later field addition does not break every caller's struct literal.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct ProxyConfig {
    /// [`TickInput`] slot the proxy reads. Must be below [`MAX_INPUT_SLOTS`].
    pub slot: usize,
    /// Spawn position.
    pub start: Vec2,
    /// World units covered per tick at full analogue deflection.
    pub speed_per_tick: f32,
    /// World-space bounds the proxy position is clamped to every tick. `min` must be less than or
    /// equal to `max` on both axes, and every component must be finite.
    pub bounds: Aabb,
    /// Radius of the graze ring's inner circle (never overlapped for a hit).
    pub graze_inner_radius: f32,
    /// Radius of the graze ring's outer circle (must be overlapped for a hit).
    pub graze_outer_radius: f32,
    /// Layers the proxy's graze probe considers.
    pub graze_mask: LayerMask,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            slot: 0,
            start: Vec2::ZERO,
            speed_per_tick: 0.25,
            bounds: Aabb {
                min: Vec2::new(-64.0, -64.0),
                max: Vec2::new(64.0, 64.0),
            },
            graze_inner_radius: 0.5,
            graze_outer_radius: 2.0,
            graze_mask: LayerMask::ALL,
        }
    }
}
impl_stable_hash!(ProxyConfig {
    slot,
    start,
    speed_per_tick,
    bounds,
    graze_inner_radius,
    graze_outer_radius,
    graze_mask
});

impl ProxyConfig {
    /// Whether every invariant this type documents holds.
    fn is_valid(&self) -> bool {
        self.slot < MAX_INPUT_SLOTS
            && self.bounds.min.x.is_finite()
            && self.bounds.min.y.is_finite()
            && self.bounds.max.x.is_finite()
            && self.bounds.max.y.is_finite()
            && self.bounds.min.x <= self.bounds.max.x
            && self.bounds.min.y <= self.bounds.max.y
    }
}

/// Name [`PlayerProxyPlugin`] registers as (contract §9.1 system-naming convention).
pub const PROXY_PLUGIN_NAME: &str = "grimoire.player_proxy";
/// Name of the parallel system that moves the proxy every tick.
pub const PROXY_MOVE_SYSTEM: &str = "fixtures.proxy_move";

/// Minimal, spawn-once player proxy: follow target, aim anchor and graze-ring centre (contract
/// §9.5). At most one proxy exists per [`Simulation`]; [`PlayerProxyPlugin::build`] panics if one
/// already does.
#[derive(Debug, Clone, Copy, Default)]
pub struct PlayerProxyPlugin {
    config: ProxyConfig,
}

impl PlayerProxyPlugin {
    /// Creates a plugin with `config`.
    ///
    /// # Panics
    /// If `config.slot >= `[`MAX_INPUT_SLOTS`], or `config.bounds` is not finite with `min <= max`
    /// on both axes.
    #[must_use]
    pub fn new(config: ProxyConfig) -> Self {
        assert!(
            config.slot < MAX_INPUT_SLOTS,
            "ProxyConfig::slot {} must be below MAX_INPUT_SLOTS ({MAX_INPUT_SLOTS})",
            config.slot
        );
        assert!(
            config.is_valid(),
            "ProxyConfig::bounds {:?} must be finite with min <= max on both axes",
            config.bounds
        );
        Self { config }
    }
}

impl GamePlugin for PlayerProxyPlugin {
    fn name(&self) -> &str {
        PROXY_PLUGIN_NAME
    }

    fn build(&mut self, sim: &mut Simulation) {
        let config = self.config;
        let world = sim.world_mut();
        assert!(proxy_entity(world).is_none(), "player proxy already exists");

        world.insert_resource(config);
        world.insert_resource(AimTarget(Some(config.start)));
        world.insert_resource(GrazeProbe {
            ring: GrazeRing {
                center: config.start,
                inner_radius: config.graze_inner_radius,
                outer_radius: config.graze_outer_radius,
            },
            mask: config.graze_mask,
        });
        world.spawn((
            PlayerProxy,
            ProxyPosition(config.start),
            ProxyPreviousPosition(config.start),
            ProxyAim([0, 0]),
        ));

        sim.schedule_mut().add_parallel_system(parallel_system_fn(
            PROXY_MOVE_SYSTEM,
            Access::new()
                .read::<ProxyPosition>()
                .read_resource::<TickInput>()
                .read_resource::<ProxyConfig>()
                .write::<ProxyPosition>()
                .write::<ProxyPreviousPosition>()
                .write::<ProxyAim>()
                .write_resource::<AimTarget>()
                .write_resource::<GrazeProbe>(),
            proxy_move,
        ));
    }
}

/// The `fixtures.proxy_move` system body (contract §9.5 "Bewegung je Tick"/"Wirkung").
///
/// No structural commands, no randomness, no clock: a pure function of `TickInput` and the
/// proxy's own previous position, applied through `commands` so the stage's other systems still
/// see the unchanged world.
fn proxy_move(world: &World, commands: &mut grimoire_ecs::CommandBuffer) {
    let config = world.resource::<ProxyConfig>().copied().unwrap_or_default();
    let input = world.resource::<TickInput>().copied().unwrap_or_default();
    let frame = input.slots[config.slot];

    for (entity, position, _) in world.query::<(Entity, &ProxyPosition, With<PlayerProxy>)>() {
        let mut v = Vec2::new(frame.axis(0), frame.axis(1));
        if v.length_squared() > 1.0 {
            v = v / dmath::sqrt(v.length_squared());
        }
        let moved = position.0 + v * config.speed_per_tick;
        let clamped = Vec2::new(
            moved.x.clamp(config.bounds.min.x, config.bounds.max.x),
            moved.y.clamp(config.bounds.min.y, config.bounds.max.y),
        );

        commands.set(entity, ProxyPreviousPosition(position.0));
        commands.set(entity, ProxyPosition(clamped));
        commands.set(entity, ProxyAim([frame.axes[2], frame.axes[3]]));
        commands.insert_resource(AimTarget(Some(clamped)));
        commands.insert_resource(GrazeProbe {
            ring: GrazeRing {
                center: clamped,
                inner_radius: config.graze_inner_radius,
                outer_radius: config.graze_outer_radius,
            },
            mask: config.graze_mask,
        });
    }
}

/// The proxy entity of `world`, if [`PlayerProxyPlugin::build`] has run.
///
/// A read-only lookup, usable as the camera follow target (interpolate
/// `ProxyPreviousPosition.lerp(ProxyPosition, alpha)`, contract §9.5 "Präsentation") and as the
/// graze-ring centre, without a declared `grimoire_ecs::Access` (contract: `Entity` and `With`
/// need none).
#[must_use]
pub fn proxy_entity(world: &World) -> Option<Entity> {
    world
        .query::<(Entity, With<PlayerProxy>)>()
        .next()
        .map(|(entity, ())| entity)
}
