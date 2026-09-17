//! The [`GamePlugin`] extension point and the per-frame [`FrameStats`].

use std::sync::Arc;
use std::time::Duration;

use grimoire_core::Vec2;
use grimoire_ecs::World;
use grimoire_platform::{PlatformWindow, RawInputEvent};
use grimoire_render::{RenderFrame, RenderStats, StageFrame};
use grimoire_sim::Simulation;

use crate::render_assets::{PluginError, RenderAssets};

/// A game (or a part of one) plugged into the engine.
///
/// Call order during a run:
/// 1. [`GamePlugin::build`] once per plugin, in registration order, on a fresh simulation.
/// 2. [`GamePlugin::register_assets`] once per plugin, in registration order, with the loop's
///    renderer (every frame loop, never [`crate::AppBuilder::run_headless`]).
/// 3. [`GamePlugin::window_created`] once per plugin on the desktop, after the renderer exists.
/// 4. Per frame: [`GamePlugin::presentation_input`] for every event that arrived since the last
///    frame, then all simulation ticks due, then [`GamePlugin::extract`] and
///    [`GamePlugin::extract_stage`] for every plugin in registration order, rendering, then
///    [`GamePlugin::on_frame`] for every plugin, then [`GamePlugin::on_profile`] for every plugin
///    (unless the profiler is off).
/// 5. [`GamePlugin::shutdown`] once per plugin, in registration order, when the frame loop ends.
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

    /// Registers the meshes and textures this plugin draws with the loop's renderer (contract
    /// §9.2, PO decision 2026-09-17 "Fassaden-Haken für Assets").
    ///
    /// Called once per run, for every plugin in registration order, right after every plugin's
    /// [`GamePlugin::build`] and before [`GamePlugin::window_created`] and the first frame: with
    /// the window or offscreen renderer in [`crate::AppBuilder::run`] and
    /// [`crate::AppBuilder::run_offscreen`], with a [`crate::HeadlessRenderAssets`] (validation
    /// and handles without a GPU) in [`crate::AppBuilder::run_headless_frames`], never in
    /// [`crate::AppBuilder::run_headless`]. Keep the returned handles in the plugin and use them
    /// in [`GamePlugin::extract_stage`]; loading figures from a pack goes through
    /// [`crate::adapters::figure_assets::load_figure_into`].
    ///
    /// Presentation only: the plugin sees no simulation here, and no handle may reach it.
    ///
    /// # Errors
    /// Any [`PluginError`]. The loop then calls no further `register_assets` or
    /// `window_created`, runs no frame, calls [`GamePlugin::shutdown`] for every plugin and ends
    /// the run with [`crate::GrimoireError::Assets`].
    fn register_assets(&mut self, assets: &mut dyn RenderAssets) -> Result<(), PluginError> {
        let _ = assets;
        Ok(())
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

    /// Receives raw keyboard and mouse events that only affect presentation (contract §9.12):
    /// camera presets, debug views, a screenshot key. The event reaches this plugin and nothing
    /// else — there is no path from here into a tick, a world hash, a snapshot or a replay, so a
    /// recording made while these keys are pressed is byte-identical to one without them.
    ///
    /// Gameplay input keeps its own path: bind it in the [`crate::InputMap`], where it becomes
    /// part of the `TickInput` the simulation reads and a recording stores.
    ///
    /// Called once per event, for every plugin in registration order, at the start of the frame
    /// after the event arrived (contract §9.3 step 0): before the timestep advances, before every
    /// tick of that frame and before [`GamePlugin::extract`]. Every frame delivers, including one
    /// without ticks, so a paused or single-stepped game keeps reacting. Events are delivered once
    /// each, in arrival order, and never coalesced. The loop drops events only when more than
    /// 4,096 pile up before a frame, and logs how many.
    ///
    /// Only the frame loops deliver ([`crate::AppBuilder::run`],
    /// [`crate::AppBuilder::run_offscreen`], [`crate::AppBuilder::run_headless_frames`] and its
    /// event-scripted form); [`crate::AppBuilder::run_headless`] has no platform events at all.
    /// Losing focus is not an input event: the loop releases held keys itself and this hook sees
    /// nothing.
    fn presentation_input(&mut self, event: &RawInputEvent) {
        let _ = event;
    }

    /// Receives the measurements of the frame that was just rendered.
    fn on_frame(&mut self, stats: &FrameStats) {
        let _ = stats;
    }

    /// Receives the profile of the frame that was just rendered, after every plugin's
    /// [`GamePlugin::on_frame`] (contract §9.2, §9.7, plan 0002 WP6.3): time per scope with
    /// budgets and estimate flags, plus the bullet counters. Scope names are the loop scopes of
    /// [`crate::adapters::debug::LOOP_SCOPES`] and the subsystem of each system name
    /// ([`crate::adapters::debug::subsystem_scope`]). Not called when
    /// [`crate::AppBuilder::profiler`] turned the profiler off.
    ///
    /// Presentation only, like `on_frame`. [`crate::adapters::debug::stats_frame`] turns the
    /// frame's [`FrameStats`] into the frame values an export or a `Stats` message needs.
    fn on_profile(&mut self, profile: &grimoire_debug::FrameProfile) {
        let _ = profile;
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
