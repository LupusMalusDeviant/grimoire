//! P1 additions to the facade `grimoire` (contract §9.1–§9.8). `GamePlugin` is mirrored: P0
//! methods verbatim plus the P1 provided methods.

use std::sync::Arc;
use std::time::Duration;

use grimoire::FrameStats;
use grimoire_core::Vec2;
use grimoire_core::math::dmath;
use grimoire_ecs::World;
use grimoire_platform::{KeyCode, PlatformWindow, RawInputEvent};
use grimoire_render::RenderFrame;
use grimoire_render_p1::StageFrame;
#[cfg(feature = "wp2-2-camera")]
use grimoire_render_p1::placeholders::Camera25D;
use grimoire_sim::Simulation;

pub use grimoire_assets as assets;
pub use grimoire_collide as collide;
pub use grimoire_debug as debug;
pub use grimoire_sigil as sigil;

// ---- §9 / §9.2 --------------------------------------------------------------------------------

pub trait GamePlugin {
    fn name(&self) -> &str;
    fn build(&mut self, sim: &mut Simulation) {
        let _ = sim;
    }
    fn extract(&mut self, world: &World, alpha: f32, frame: &mut RenderFrame) {
        let _ = (world, alpha, frame);
    }
    fn on_frame(&mut self, stats: &FrameStats) {
        let _ = stats;
    }
    fn window_created(&mut self, window: &Arc<dyn PlatformWindow>) {
        let _ = window;
    }
    fn shutdown(&mut self) {}

    fn extract_stage(&mut self, world: &World, alpha: f32, stage: &mut StageFrame) {
        let _ = (world, alpha, stage);
    }
    fn focus(&self, world: &World, alpha: f32) -> Option<Vec2> {
        let _ = (world, alpha);
        None
    }
    fn on_profile(&mut self, profile: &grimoire_debug::FrameProfile) {
        let _ = profile;
    }
}

/// Stand-in for the P1 `AppBuilder` methods.
pub trait AppBuilderP1: Sized {
    fn profiler(self, enabled: bool) -> Self;
    fn overlay_key(self, key: Option<KeyCode>) -> Self;
    #[cfg(feature = "debug-link")]
    fn debug_link(self, transport: Box<dyn grimoire_debug::DebugTransport>) -> Self;
}

impl AppBuilderP1 for grimoire::AppBuilder {
    fn profiler(self, enabled: bool) -> Self {
        let _ = enabled;
        unimplemented!()
    }
    fn overlay_key(self, key: Option<KeyCode>) -> Self {
        let _ = (key, KeyCode::F3);
        unimplemented!()
    }
    #[cfg(feature = "debug-link")]
    fn debug_link(self, transport: Box<dyn grimoire_debug::DebugTransport>) -> Self {
        let _ = transport;
        unimplemented!()
    }
}

#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct PointerState {
    position: Option<[f32; 2]>,
}

impl PointerState {
    pub fn apply(&mut self, event: &RawInputEvent) {
        if let RawInputEvent::CursorMoved { x, y } = *event {
            self.position = Some([x as f32, y as f32]);
        }
    }
    pub fn position(&self) -> Option<[f32; 2]> {
        self.position
    }
}

pub const AIM_MIN_DISTANCE: f32 = 0.01;

// §9.4 code block, verbatim.
pub fn quantize_aim(offset: Vec2) -> [i16; 2] {
    if !offset.x.is_finite() || !offset.y.is_finite() { return [0, 0]; }
    let m = dmath::max(offset.x.abs(), offset.y.abs());
    if m < AIM_MIN_DISTANCE { return [0, 0]; }
    let s = offset / m;
    let n = s / dmath::sqrt(s.length_squared());
    [(n.x * 32767.0).round() as i16, (n.y * 32767.0).round() as i16]
}
#[cfg(feature = "wp2-2-camera")]
pub fn sample_aim(camera: &Camera25D, cursor: [f32; 2], viewport: [f32; 2], focus: Vec2) -> Option<[i16; 2]> {
    let [gx, gy] = camera.screen_to_ground(cursor, viewport)?;
    Some(quantize_aim(Vec2::new(gx, gy) - focus))
}

// ---- §9.1 / §9.6 adapters ---------------------------------------------------------------------

pub mod adapters {
    pub mod sigil_render {}
    pub mod assets {}

    pub mod debug {
        use grimoire::FrameStats;
        use grimoire_debug::{FrameProfile, Message, ScopeId, StatsFrame, SwapAck};
        use grimoire_ecs::World;
        use grimoire_ecs_p1::{SystemInfo, SystemObserver};
        use grimoire_platform::Clock;

        /// §9.7: ScopeIds keyed by the name prefix before the first '.', in first-seen order.
        #[derive(Default, Debug)]
        pub struct ScopeTable(Vec<String>);

        impl ScopeTable {
            pub fn scope_of<'n>(&mut self, system_name: &'n str) -> (ScopeId, &'n str) {
                let prefix = match system_name.split_once('.') {
                    Some((prefix, _)) => prefix,
                    None => "app",
                };
                let index = match self.0.iter().position(|name| name == prefix) {
                    Some(index) => index,
                    None => {
                        self.0.push(prefix.to_owned());
                        self.0.len() - 1
                    }
                };
                (ScopeId(u16::try_from(index).unwrap_or(u16::MAX)), prefix)
            }
        }

        /// §9.7 profiler observer: borrows the clock, records per exclusive system.
        pub struct ProfilerObserver<'a> {
            pub clock: &'a dyn Clock,
            pub profile: &'a mut FrameProfile,
            pub scopes: &'a mut ScopeTable,
            pub started: std::time::Duration,
        }

        impl SystemObserver for ProfilerObserver<'_> {
            fn system_started(&mut self, system: SystemInfo<'_>) {
                let _ = system;
                self.started = self.clock.elapsed();
            }

            fn system_finished(&mut self, system: SystemInfo<'_>, world: &World) {
                let _ = world;
                let elapsed = self.clock.elapsed().saturating_sub(self.started);
                let (scope, name) = self.scopes.scope_of(system.name);
                self.profile.record(scope, name, elapsed);
            }
        }

        /// §9.7 / §13: the facade builds `StatsFrame` outside `grimoire_debug`.
        pub fn stats_message(profile: &FrameProfile, stats: &FrameStats, swaps: u32, manifest: u64) -> Message {
            let mut frame = StatsFrame::default();
            frame.frame = stats.frame;
            frame.sim_tick = stats.sim_tick;
            frame.ticks_this_frame = stats.ticks_this_frame;
            frame.alpha = stats.alpha;
            frame.frame_time = stats.frame_time;
            frame.fps = stats.fps as f32; // FrameStats::fps is f64 (§9), the wire field is f32 (§13)
            frame.dropped_time = stats.dropped_time;
            frame.content_swaps = swaps;
            frame.content_manifest = manifest;
            Message::Stats(profile.to_stats(&frame))
        }

        /// §9.7: `SwapAck` from a `SwapReport`, built outside `grimoire_debug`.
        pub fn swap_ack(report: &grimoire_sigil::SwapReport, in_reply_to: u32) -> Message {
            let mut ack = SwapAck::default();
            ack.in_reply_to = in_reply_to;
            ack.applied_tick = report.effective_tick;
            ack.content_swaps = report.epoch.swaps;
            Message::SwapAck(ack)
        }
    }

    pub mod sigil_collide {
        use grimoire_collide::{
            CollisionQuery, GrazeRing, GridConfig, GridItem, Hit, LayerMask, Shape, SpatialGrid,
        };
        use grimoire_core::{StableHash, StableHasher, Vec2, impl_stable_hash};
        use grimoire_ecs::{Access, CommandBuffer, Entity, World, parallel_system_fn, system_fn};
        use grimoire_sigil::{BulletPool, SigilContent};
        use grimoire_sim::Simulation;

        use crate::GamePlugin;

        #[non_exhaustive]
        #[derive(Clone, Copy, PartialEq, Debug)]
        pub struct SigilCollideConfig {
            pub grid: GridConfig,
            pub bullet_layers: LayerMask,
        }
        impl_stable_hash!(SigilCollideConfig {
            grid,
            bullet_layers
        });

        impl Default for SigilCollideConfig {
            fn default() -> Self {
                Self {
                    grid: GridConfig::new(Vec2::new(-256.0, -256.0), 4.0, 128, 128),
                    bullet_layers: LayerMask::layer(0),
                }
            }
        }

        #[derive(Clone, Copy, Default, PartialEq, Debug)]
        pub struct GrazeProbe {
            pub ring: GrazeRing,
            pub mask: LayerMask,
        }
        impl_stable_hash!(GrazeProbe { ring, mask });

        #[derive(Clone, Default, PartialEq, Debug)]
        pub struct GrazeHits(pub Vec<Hit>);
        impl StableHash for GrazeHits {
            fn stable_hash(&self, hasher: &mut StableHasher) {
                self.0.stable_hash(hasher);
            }
        }

        #[derive(Default)]
        pub struct SigilCollidePlugin {
            config: SigilCollideConfig,
        }

        impl SigilCollidePlugin {
            pub fn new(config: SigilCollideConfig) -> Self {
                Self { config }
            }
        }

        pub const BROADPHASE_SYSTEM: &str = "collide.broadphase";
        pub const GRAZE_SYSTEM: &str = "collide.graze";

        fn broadphase(world: &mut World) {
            let Some(mut grid) = world.remove_resource::<SpatialGrid>() else {
                return;
            };
            let Some(config) = world.resource::<SigilCollideConfig>().copied() else {
                return;
            };
            let content = world.resource::<SigilContent>();
            let pool_items = world
                .resource::<BulletPool>()
                .into_iter()
                .flat_map(|pool| pool.iter())
                .filter_map(|bullet| {
                    let unit = content?.library().units().get(usize::from(bullet.unit_index()))?;
                    let ty = unit.bullet_types().get(usize::from(bullet.bullet_type()))?;
                    let id = bullet.id();
                    Some(GridItem {
                        key: grimoire_collide::ColliderKey::pool(id.index(), id.generation()),
                        shape: Shape::Circle(grimoire_collide::Circle {
                            center: bullet.position(),
                            radius: ty.collision_radius,
                        }),
                        layers: config.bullet_layers,
                    })
                });
            let entity_items = world
                .query::<(Entity, &grimoire_collide::Collider)>()
                .map(|(entity, collider)| GridItem {
                    key: grimoire_collide::ColliderKey::from_entity(entity),
                    shape: collider.shape,
                    layers: collider.layers,
                });
            let items: Vec<GridItem> = pool_items.chain(entity_items).collect();
            grid.rebuild_par(world.executor(), items);
            world.insert_resource(grid);
        }

        fn graze(world: &World, commands: &mut CommandBuffer) {
            let (Some(grid), Some(probe)) = (world.resource::<SpatialGrid>(), world.resource::<GrazeProbe>()) else {
                return;
            };
            let mut hits = Vec::new();
            grid.graze_ring(&probe.ring, probe.mask, &mut hits);
            commands.insert_resource(GrazeHits(hits));
        }

        impl GamePlugin for SigilCollidePlugin {
            fn name(&self) -> &str {
                "grimoire.sigil_collide"
            }

            fn build(&mut self, sim: &mut Simulation) {
                let grid = SpatialGrid::new(self.config.grid).unwrap_or_else(|error| panic!("{error}"));
                let world = sim.world_mut();
                world.insert_resource(self.config);
                world.insert_resource(grid);
                world.insert_resource(GrazeHits::default());
                if world.resource::<GrazeProbe>().is_none() {
                    world.insert_resource(GrazeProbe::default());
                }
                sim.schedule_mut()
                    .add_system(system_fn(BROADPHASE_SYSTEM, broadphase))
                    .add_parallel_system(parallel_system_fn(
                        GRAZE_SYSTEM,
                        Access::new()
                            .read_resource::<SpatialGrid>()
                            .read_resource::<GrazeProbe>()
                            .write_resource::<GrazeHits>(),
                        graze,
                    ));
            }
        }
    }
}

// ---- §9.5 fixtures ----------------------------------------------------------------------------

#[cfg(feature = "fixtures")]
pub mod fixtures {
    use grimoire_collide::{Aabb, GrazeRing, LayerMask};
    use grimoire_core::math::dmath;
    use grimoire_core::{StableHash, StableHasher, Vec2, impl_stable_hash};
    use grimoire_ecs::{Access, CommandBuffer, Entity, With, World, parallel_system_fn};
    use grimoire_render::{SpriteInstance, shape};
    use grimoire_render_p1::StageFrame;
    use grimoire_sigil::AimTarget;
    use grimoire_sim::{MAX_INPUT_SLOTS, Simulation, TickInput};

    use crate::GamePlugin;
    use crate::adapters::sigil_collide::GrazeProbe;

    #[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
    pub struct PlayerProxy;
    impl StableHash for PlayerProxy {
        fn stable_hash(&self, _hasher: &mut StableHasher) {}
    }

    #[derive(Clone, Copy, Default, PartialEq, Debug)]
    pub struct ProxyPosition(pub Vec2);
    impl StableHash for ProxyPosition {
        fn stable_hash(&self, hasher: &mut StableHasher) {
            self.0.stable_hash(hasher);
        }
    }

    #[derive(Clone, Copy, Default, PartialEq, Debug)]
    pub struct ProxyPreviousPosition(pub Vec2);
    impl StableHash for ProxyPreviousPosition {
        fn stable_hash(&self, hasher: &mut StableHasher) {
            self.0.stable_hash(hasher);
        }
    }

    #[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
    pub struct ProxyAim(pub [i16; 2]);
    impl StableHash for ProxyAim {
        fn stable_hash(&self, hasher: &mut StableHasher) {
            self.0.stable_hash(hasher);
        }
    }

    #[non_exhaustive]
    #[derive(Clone, Copy, PartialEq, Debug)]
    pub struct ProxyConfig {
        pub slot: usize,
        pub start: Vec2,
        pub speed_per_tick: f32,
        pub bounds: Aabb,
        pub graze_inner_radius: f32,
        pub graze_outer_radius: f32,
        pub graze_mask: LayerMask,
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

    #[derive(Default)]
    pub struct PlayerProxyPlugin {
        config: ProxyConfig,
    }

    impl PlayerProxyPlugin {
        pub fn new(config: ProxyConfig) -> Self {
            assert!(config.slot < MAX_INPUT_SLOTS, "invalid proxy config");
            Self { config }
        }
    }

    pub const PROXY_PLUGIN_NAME: &str = "grimoire.player_proxy";
    pub const PROXY_MOVE_SYSTEM: &str = "fixtures.proxy_move";

    pub fn proxy_entity(world: &World) -> Option<Entity> {
        world
            .query::<(Entity, With<PlayerProxy>)>()
            .next()
            .map(|(entity, ())| entity)
    }

    fn proxy_move(world: &World, commands: &mut CommandBuffer) {
        let (Some(config), Some(input)) = (world.resource::<ProxyConfig>(), world.resource::<TickInput>()) else {
            return;
        };
        let f = input.slots[config.slot];
        for (entity, position, ()) in world.query::<(Entity, &ProxyPosition, With<PlayerProxy>)>() {
            let p = position.0;
            let mut v = Vec2::new(f.axis(0), f.axis(1));
            if v.length_squared() > 1.0 {
                v = v / dmath::sqrt(v.length_squared());
            }
            let moved = p + v * config.speed_per_tick;
            let next = Vec2::new(
                moved.x.clamp(config.bounds.min.x, config.bounds.max.x),
                moved.y.clamp(config.bounds.min.y, config.bounds.max.y),
            );
            commands.set(entity, ProxyPreviousPosition(p));
            commands.set(entity, ProxyPosition(next));
            commands.set(entity, ProxyAim([f.axes[2], f.axes[3]]));
            commands.insert_resource(AimTarget(Some(next)));
            commands.insert_resource(GrazeProbe {
                ring: GrazeRing {
                    center: next,
                    inner_radius: config.graze_inner_radius,
                    outer_radius: config.graze_outer_radius,
                },
                mask: config.graze_mask,
            });
        }
    }

    impl GamePlugin for PlayerProxyPlugin {
        fn name(&self) -> &str {
            PROXY_PLUGIN_NAME
        }

        fn build(&mut self, sim: &mut Simulation) {
            assert!(proxy_entity(sim.world()).is_none(), "player proxy already exists");
            let start = self.config.start;
            let world = sim.world_mut();
            world.insert_resource(self.config);
            world.insert_resource(AimTarget(Some(start)));
            world.insert_resource(GrazeProbe {
                ring: GrazeRing {
                    center: start,
                    inner_radius: self.config.graze_inner_radius,
                    outer_radius: self.config.graze_outer_radius,
                },
                mask: self.config.graze_mask,
            });
            world.spawn((
                PlayerProxy,
                ProxyPosition(start),
                ProxyPreviousPosition(start),
                ProxyAim([0, 0]),
            ));
            // §9.5 access block, verbatim.
            let access = Access::new()
                .read::<ProxyPosition>()
                .read_resource::<TickInput>()
                .read_resource::<ProxyConfig>()
                .write::<ProxyPosition>()
                .write::<ProxyPreviousPosition>()
                .write::<ProxyAim>()
                .write_resource::<AimTarget>()
                .write_resource::<GrazeProbe>();
            sim.schedule_mut()
                .add_parallel_system(parallel_system_fn(PROXY_MOVE_SYSTEM, access, proxy_move));
        }

        fn focus(&self, world: &World, alpha: f32) -> Option<Vec2> {
            let entity = proxy_entity(world)?;
            let previous = world.get::<ProxyPreviousPosition>(entity)?.0;
            let current = world.get::<ProxyPosition>(entity)?.0;
            Some(previous.lerp(current, alpha))
        }

        fn extract_stage(&mut self, world: &World, alpha: f32, stage: &mut StageFrame) {
            if let Some(center) = self.focus(world, alpha) {
                stage.marker_sprites.push(SpriteInstance {
                    position: center.to_array(),
                    half_size: [0.5, 0.5],
                    rotation: 0.0,
                    shape: shape::CIRCLE,
                    color: [1.0, 1.0, 1.0, 1.0],
                });
            }
        }
    }
}

pub mod prelude {
    pub use crate::adapters::sigil_collide::GrazeHits;
    pub use crate::{AppBuilderP1, GamePlugin};
    pub use grimoire::prelude::*;
    pub use grimoire_collide::{Collider, CollisionQuery, LayerMask, Shape};
    pub use grimoire_render_p1::StageFrame;
    #[cfg(feature = "wp2-2-camera")]
    pub use grimoire_render_p1::placeholders::Camera25D;
    pub use grimoire_sigil::{AimTarget, ClearFilter, ClearRequest, Emitter, SigilConfig, UnitId};
}

/// Keep `Duration` referenced for the profiler scope.
pub const FRAME_SCOPE_BUDGET: Duration = Duration::from_millis(16);
