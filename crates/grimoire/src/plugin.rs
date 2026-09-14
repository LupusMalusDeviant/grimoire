//! The [`GamePlugin`] extension point and the per-frame [`FrameStats`].

use std::sync::Arc;
use std::time::Duration;

use grimoire_ecs::World;
use grimoire_platform::PlatformWindow;
use grimoire_render::{RenderFrame, RenderStats};
use grimoire_sim::Simulation;

/// A game (or a part of one) plugged into the engine.
///
/// Call order during a run:
/// 1. [`GamePlugin::build`] once per plugin, in registration order, on a fresh simulation.
/// 2. [`GamePlugin::window_created`] once per plugin on the desktop, after the renderer exists.
/// 3. Per frame: all simulation ticks due, then [`GamePlugin::extract`] for every plugin in
///    registration order, rendering, then [`GamePlugin::on_frame`] for every plugin.
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

    /// Receives the measurements of the frame that was just rendered.
    fn on_frame(&mut self, stats: &FrameStats) {
        let _ = stats;
    }

    /// Receives the main window once it and the renderer exist (desktop runs only), e.g. to
    /// update the title from [`GamePlugin::on_frame`].
    fn window_created(&mut self, window: &Arc<dyn PlatformWindow>) {
        let _ = window;
    }
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
