//! # grimoire
//!
//! Facade of the Grimoire engine. Games depend on this crate: it owns the application
//! lifecycle ([`App`], [`GamePlugin`], the fixed-timestep main loop, [`InputMap`]) and re-exports
//! what a game needs in [`prelude`] and the engine crates as modules.
//!
//! A game that runs the simulation on several threads also depends on `grimoire_exec` and passes
//! its `ThreadPoolExecutor` to [`AppBuilder::executor`]. The facade itself creates no threads and
//! does not depend on `grimoire_exec` (engine ADR-0006); every state hash is the same with any
//! executor.
//!
//! ## Main loop
//!
//! Every frame of [`AppBuilder::run`] (and of [`AppBuilder::run_headless_frames`], which drives
//! the same loop with a [`grimoire_render::NullRenderer`] and a manual clock):
//!
//! Once per run, after every plugin's [`GamePlugin::build`], each plugin registers its meshes and
//! textures with the loop's renderer through [`GamePlugin::register_assets`] ([`RenderAssets`]),
//! so a game never needs a loop of its own to load figures from a pack.
//!
//! 1. Platform events: resizes reach the renderer, raw input updates the held keys and buttons
//!    and latches every press, focus loss releases everything held.
//! 2. The frame time from the platform clock advances a [`grimoire_sim::FixedTimestep`]. The
//!    [`InputMap`] is sampled once into input slot 0, held or latched inputs count as pressed,
//!    and the simulation steps once per tick due with that input. The latch is cleared only after
//!    a frame that ran at least one tick, so a tap released between two ticks is not lost.
//! 3. Every plugin extracts sprites and the WP2.2 stage channels ([`GamePlugin::extract`], then
//!    [`GamePlugin::extract_stage`]) with the interpolation factor `alpha`. The first plugin whose
//!    [`GamePlugin::focus`] returns `Some` drives the render-side camera follow spring
//!    ([`grimoire_render::CameraFollow`], set up via [`AppBuilder::camera25d`], plan 0002 WP2.4)
//!    and next frame's mouse-aim sampling ([`sample_aim`], contract §9.4) — never the simulation,
//!    which only ever sees the resulting quantised axes. If the overlay key (default F3) has
//!    toggled the stats overlay on, the loop adds it to the debug channel
//!    ([`adapters::debug::StatsOverlay`], plan 0002 WP6.4). The renderer then draws the stage.
//!    A lost surface skips the frame and is reported to the platform, which throttles the loop
//!    while frames keep going unpresented; any other render error ends the run with that error.
//! 4. Every plugin receives the [`FrameStats`], then the frame's profile
//!    ([`GamePlugin::on_profile`]): time per loop scope (`frame`, `sim`, `extract`, `render`,
//!    `gpu`) and per subsystem, measured with the platform clock, with budgets and the bullet
//!    counters ([`adapters::debug`], plan 0002 WP6.3). [`AppBuilder::profiler`] turns it off.
//!
//! When the loop ends, every plugin receives [`GamePlugin::shutdown`] once, in registration order.
//!
//! The clock is read only by the loop, never by the simulation, so the same seed and the same
//! per-tick input give the same state hashes on the desktop, in the frame loop and in
//! [`AppBuilder::run_headless`].
//!
//! ## Hello Grimoire
//!
//! ```
//! use grimoire::prelude::*;
//!
//! #[derive(Clone)]
//! struct Position {
//!     at: Vec2,
//! }
//! impl_stable_hash!(Position { at });
//!
//! struct Hello;
//!
//! impl GamePlugin for Hello {
//!     fn name(&self) -> &str {
//!         "hello"
//!     }
//!
//!     fn build(&mut self, sim: &mut Simulation) {
//!         sim.world_mut().spawn((Position { at: Vec2::ZERO },));
//!         sim.schedule_mut().add_system(system_fn("walk", |world| {
//!             let input = world.resource::<TickInput>().copied().unwrap_or_default();
//!             let step = Vec2::new(input.slots[0].axis(0), input.slots[0].axis(1));
//!             for position in world.query_mut::<&mut Position>() {
//!                 position.at += step;
//!             }
//!         }));
//!     }
//!
//!     fn extract(&mut self, world: &World, _alpha: f32, frame: &mut RenderFrame) {
//!         for position in world.query::<&Position>() {
//!             frame.sprites.push(SpriteInstance {
//!                 position: position.at.to_array(),
//!                 half_size: [1.0, 1.0],
//!                 shape: shape::CIRCLE,
//!                 color: [1.0, 0.8, 0.2, 1.0],
//!                 ..SpriteInstance::default()
//!             });
//!         }
//!     }
//! }
//!
//! // Desktop: App::new(WindowConfig::default()).plugin(Hello).run()?
//! let report = App::new(WindowConfig::default())
//!     .seed(42)
//!     .plugin(Hello)
//!     .run_headless(120, &mut |_tick| TickInput::default());
//! assert_eq!(report.final_tick, 120);
//! ```

pub mod adapters;
mod aim;
mod app;
mod error;
#[cfg(feature = "fixtures")]
pub mod fixtures;
mod input;
mod main_loop;
mod plugin;
mod render_assets;

pub use aim::{AIM_MIN_DISTANCE, PointerState, quantize_aim, sample_aim};
pub use app::{
    App, AppBuilder, DEFAULT_HASH_EVERY, DEFAULT_MAX_TICKS_PER_FRAME, DEFAULT_TICK_RATE_HZ,
    HeadlessReport, OffscreenRun,
};
pub use error::GrimoireError;
pub use input::{
    AXIS_COUNT, AXIS_MAX, BUTTON_COUNT, InputAction, InputMap, InputSource, InputState,
};
pub use main_loop::LoopReport;
pub use plugin::{FrameStats, GamePlugin};
pub use render_assets::{HeadlessRenderAssets, PluginError, RenderAssets};

pub use grimoire_assets as assets;
pub use grimoire_collide as collide;
pub use grimoire_core as core;
pub use grimoire_debug as debug;
pub use grimoire_ecs as ecs;
pub use grimoire_platform as platform;
pub use grimoire_render as render;
pub use grimoire_sigil as sigil;
pub use grimoire_sim as sim;

/// Everything a game typically needs, for `use grimoire::prelude::*;`.
pub mod prelude {
    pub use crate::{
        App, AppBuilder, FrameStats, GamePlugin, GrimoireError, InputAction, InputMap, InputSource,
    };
    pub use grimoire_core::math::dmath;
    pub use grimoire_core::{StableHash, StableHasher, Vec2, impl_stable_hash};
    pub use grimoire_ecs::{
        Access, CommandBuffer, Entity, Executor, ParallelSystem, QueryBlock, Schedule,
        SequentialExecutor, World, parallel_system_fn, system_fn,
    };
    pub use grimoire_platform::{KeyCode, MouseButton, PlatformWindow, WindowConfig};
    pub use grimoire_render::{
        Camera2D, Camera25D, RenderFrame, RendererConfig, SpriteInstance, StageFrame, shape,
    };
    pub use grimoire_sim::{
        InputFrame, SimRng, SimSeed, Simulation, Tick, TickInput, derive_block_rng, derive_rng,
    };
}
