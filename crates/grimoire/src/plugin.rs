//! The [`GamePlugin`] extension point and the per-frame [`FrameStats`].

use std::sync::Arc;
use std::time::Duration;

use grimoire_core::Vec2;
use grimoire_ecs::World;
use grimoire_platform::PlatformWindow;
use grimoire_render::{RenderFrame, RenderStats, StageFrame};
use grimoire_sim::Simulation;

/// A game (or a part of one) plugged into the engine.
///
/// Call order during a run:
/// 1. [`GamePlugin::build`] once per plugin, in registration order, on a fresh simulation.
/// 2. [`GamePlugin::window_created`] once per plugin on the desktop, after the renderer exists.
/// 3. Per frame: all simulation ticks due, then [`GamePlugin::extract`] and
///    [`GamePlugin::extract_stage`] for every plugin in registration order, rendering, then
///    [`GamePlugin::on_frame`] for every plugin.
/// 4. [`GamePlugin::shutdown`] once per plugin, in registration order, when the frame loop ends.
///
/// Only `build` may shape simulation state (components, resources, systems, initial entities);
/// the other hooks are presentation and must not influence it, or runs stop being reproducible.
pub trait GamePlugin {
    /// Name for diagnostics.
    fn name(&self) -> &str;

    /// Registers components and resources, adds systems and spawns the initial entities.
    ///
    /// Systems keep all simulation state inside the world; see [`Simulation`].
    fn build(&mut self, sim: &mut Simulation) {
        let _ = sim;
    }

    /// Describes the current state for rendering. `frame` has been cleared of sprites before the
    /// first plugin runs; camera and clear colour keep the values of the previous frame.
    ///
    /// `alpha` in `[0, 1)` is the fraction of the next tick already elapsed, for interpolating
    /// between the previous and the current simulation state.
    fn extract(&mut self, world: &World, alpha: f32, frame: &mut RenderFrame) {
        let _ = (world, alpha, frame);
    }

    /// Describes the current state for the WP2.2 stage channels (contract §6): bullets, meshes,
    /// materials, lights, the marker/debug sprite layers and [`grimoire_render::Camera25D`]. Runs
    /// for every plugin, in registration order, right after every plugin's [`GamePlugin::extract`]
    /// (contract §9.3 step 5). Read-only like `extract`; must not influence simulation state.
    fn extract_stage(&mut self, world: &World, alpha: f32, stage: &mut StageFrame) {
        let _ = (world, alpha, stage);
    }

    /// The camera's follow target and mouse-aim anchor for this frame (contract §9.2/§9.5), in
    /// ground/world units, already interpolated with `alpha` like [`GamePlugin::extract`]. Called
    /// for every plugin in registration order; the first `Some` wins and the rest are not asked.
    /// `None` (the default) means this plugin has no opinion, e.g. because it manages no player
    /// character.
    ///
    /// The main loop feeds the winning point into the render-side camera follow spring
    /// ([`grimoire_render::CameraFollow`], plan 0002 WP2.4) and into [`crate::sample_aim`] — never
    /// into the simulation, so which plugin (if any) implements this cannot change a state hash.
    fn focus(&self, world: &World, alpha: f32) -> Option<Vec2> {
        let _ = (world, alpha);
        None
    }

    /// Receives the measurements of the frame that was just rendered.
    fn on_frame(&mut self, stats: &FrameStats) {
        let _ = stats;
    }

    /// Receives the main window once it and the renderer exist (desktop runs only), e.g. to
    /// update the title from [`GamePlugin::on_frame`].
    fn window_created(&mut self, window: &Arc<dyn PlatformWindow>) {
        let _ = window;
    }

    /// Called once when the frame loop of [`crate::AppBuilder::run`] or
    /// [`crate::AppBuilder::run_headless_frames`] ends after the plugins were built. Not called
    /// when the renderer could not be created, and never by [`crate::AppBuilder::run_headless`].
    ///
    /// This is the only end-of-run hook that runs on every orderly end of the loop, including
    /// quitting through the macOS application menu (Cmd+Q): that ends the process right after this
    /// call, so [`crate::AppBuilder::run`] does not return and destructors do not run. It does not
    /// run when the operating system ends the process (Windows session end, `SIGTERM` or `SIGINT`,
    /// Ctrl+C in a console). Nothing receives a result from here, so failures (for example of a
    /// final save) must be logged.
    fn shutdown(&mut self) {}
}

/// Measurements of one frame of the main loop.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameStats {
    /// Index of this frame, starting at 0.
    pub frame: u64,
    /// Simulation tick count after this frame's ticks (the index of the next tick).
    pub sim_tick: u64,
    /// Ticks simulated during this frame, at most the configured maximum per frame.
    pub ticks_this_frame: u32,
    /// Interpolation factor passed to [`GamePlugin::extract`], in `[0, 1)`.
    pub alpha: f32,
    /// Real time since the previous frame as measured by the platform clock.
    pub frame_time: Duration,
    /// Frames per second over the last completed measurement window of at least one second;
    /// `0.0` until the first window completes.
    pub fps: f64,
    /// Total real time the fixed timestep discarded because frames exceeded the tick limit.
    pub dropped_time: Duration,
    /// Renderer measurements; all zero when the frame could not be presented.
    pub render: RenderStats,
}
