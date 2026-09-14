//! # grimoire
//!
//! Facade of the Grimoire engine. Games depend on this crate only: it owns the application
//! lifecycle ([`App`], [`GamePlugin`], the fixed-timestep main loop, [`InputMap`]) and re-exports
//! what a game needs in [`prelude`] and the engine crates as modules.
//!
//! ## Main loop
//!
//! Every frame of [`AppBuilder::run`] (and of [`AppBuilder::run_headless_frames`], which drives
//! the same loop with a [`grimoire_render::NullRenderer`] and a manual clock):
//!
//! 1. Platform events: resizes reach the renderer, raw input updates the held keys and buttons,
//!    focus loss releases everything held.
//! 2. The frame time from the platform clock advances a [`grimoire_sim::FixedTimestep`]; for each
//!    tick due, the [`InputMap`] is sampled into input slot 0 and the simulation steps once.
//! 3. Every plugin extracts sprites with the interpolation factor `alpha`, the renderer draws.
//!    A lost surface skips the frame; any other render error ends the run with that error.
//! 4. Every plugin receives the [`FrameStats`].
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

mod app;
mod error;
mod input;
mod main_loop;
mod plugin;

pub use app::{
    App, AppBuilder, DEFAULT_HASH_EVERY, DEFAULT_MAX_TICKS_PER_FRAME, DEFAULT_TICK_RATE_HZ,
    HeadlessReport,
};
pub use error::GrimoireError;
pub use input::{
    AXIS_COUNT, AXIS_MAX, BUTTON_COUNT, InputAction, InputMap, InputSource, InputState,
};
pub use main_loop::LoopReport;
pub use plugin::{FrameStats, GamePlugin};

pub use grimoire_core as core;
pub use grimoire_ecs as ecs;
pub use grimoire_platform as platform;
pub use grimoire_render as render;
pub use grimoire_sim as sim;

/// Everything a game typically needs, for `use grimoire::prelude::*;`.
pub mod prelude {
    pub use crate::{
        App, AppBuilder, FrameStats, GamePlugin, GrimoireError, InputAction, InputMap, InputSource,
    };
    pub use grimoire_core::math::dmath;
    pub use grimoire_core::{StableHash, StableHasher, Vec2, impl_stable_hash};
    pub use grimoire_ecs::{CommandBuffer, Entity, Schedule, World, system_fn};
    pub use grimoire_platform::{KeyCode, MouseButton, PlatformWindow, WindowConfig};
    pub use grimoire_render::{Camera2D, RenderFrame, RendererConfig, SpriteInstance, shape};
    pub use grimoire_sim::{InputFrame, SimRng, SimSeed, Simulation, Tick, TickInput, derive_rng};
}
