//! [`App`] and its [`AppBuilder`]: configuring and starting a run.

use std::fmt;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use grimoire_ecs::Executor;
use grimoire_platform::{
    KeyCode, PlatformError, PlatformEvent, WindowConfig, run_desktop, run_headless,
};
use grimoire_render::{Camera25D, NullRenderer, RendererConfig, StageRendererConfig, WgpuRenderer};
use grimoire_sim::{Simulation, TickInput};

use crate::adapters::debug::{DEFAULT_OVERLAY_KEY, ProfilerBudgets};
use crate::error::GrimoireError;
use crate::input::InputMap;
use crate::main_loop::{
    GameLoop, LoopReport, LoopSettings, OffscreenFrames, Outcome, RendererFactory, ScriptedEvents,
};
use crate::plugin::GamePlugin;
use crate::render_assets::LoopRenderer;

/// Default simulation rate.
pub const DEFAULT_TICK_RATE_HZ: u32 = 60;

/// Default number of ticks between recorded state hashes.
pub const DEFAULT_HASH_EVERY: u64 = 60;

/// Default limit of simulation ticks per frame (see [`grimoire_sim::FixedTimestep`]).
pub const DEFAULT_MAX_TICKS_PER_FRAME: u32 = 8;

/// Entry point of a Grimoire application.
///
/// ```no_run
/// use grimoire::prelude::*;
///
/// App::new(WindowConfig::default()).seed(7).run()?;
/// # Ok::<(), grimoire::GrimoireError>(())
/// ```
#[derive(Debug, Clone, Copy)]
pub struct App;

impl App {
    /// Starts configuring an application that opens a window described by `window`.
    #[must_use]
    // A constructor-style entry point reads better than a free function in game code.
    #[allow(clippy::new_ret_no_self)]
    pub fn new(window: WindowConfig) -> AppBuilder {
        AppBuilder {
            window,
            seed: 0,
            tick_rate_hz: DEFAULT_TICK_RATE_HZ,
            max_ticks_per_frame: DEFAULT_MAX_TICKS_PER_FRAME,
            hash_every: DEFAULT_HASH_EVERY,
            input_map: InputMap::default(),
            renderer_config: StageRendererConfig::default(),
            plugins: Vec::new(),
            max_frames: None,
            exit_key: None,
            overlay_key: Some(DEFAULT_OVERLAY_KEY),
            executor: None,
            camera_25d: None,
            profiler: true,
            profiler_budgets: ProfilerBudgets::default(),
        }
    }
}

/// Settings of an offscreen run ([`AppBuilder::run_offscreen`]).
///
/// Build it with [`OffscreenRun::new`] and field assignment (contract §2 rule 13).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OffscreenRun {
    /// Width of the offscreen image in pixels.
    pub width: u32,
    /// Height of the offscreen image in pixels.
    pub height: u32,
    /// Frames to run.
    pub frames: u64,
    /// Manual clock advance before every frame.
    pub frame_delta: Duration,
    /// Every how many frames the image is read back for the capture callback, starting with frame
    /// 0; `0` (the default of [`OffscreenRun::new`]) never reads back.
    pub capture_every: u64,
}

impl OffscreenRun {
    /// `frames` frames of `width` × `height` pixels, `frame_delta` apart, without capture.
    #[must_use]
    pub fn new(width: u32, height: u32, frames: u64, frame_delta: Duration) -> Self {
        Self {
            width,
            height,
            frames,
            frame_delta,
            capture_every: 0,
        }
    }
}

/// Result of [`AppBuilder::run_headless`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadlessReport {
    /// Simulation tick count at the end of the run.
    pub final_tick: u64,
    /// [`Simulation::state_hash`] at the end of the run.
    pub final_hash: u64,
    /// `(tick, state_hash)` after every tick that is a multiple of `hash_every`, then the final
    /// state unless it was just recorded (the semantics of [`grimoire_sim::replay`]).
    pub hashes: Vec<(u64, u64)>,
}

/// Configuration of an application run; created by [`App::new`].
pub struct AppBuilder {
    window: WindowConfig,
    seed: u64,
    tick_rate_hz: u32,
    max_ticks_per_frame: u32,
    hash_every: u64,
    input_map: InputMap,
    /// Window and offscreen renderer configuration; [`AppBuilder::renderer_config`] sets `base`.
    renderer_config: StageRendererConfig,
    plugins: Vec<Box<dyn GamePlugin>>,
    max_frames: Option<u64>,
    exit_key: Option<KeyCode>,
    /// Toggles the stats overlay (WP6.4, contract §9.2; default F3).
    overlay_key: Option<KeyCode>,
    /// `None` keeps the world's default, the sequential executor.
    executor: Option<Arc<dyn Executor>>,
    /// `None` disables the WP2.4 render-side camera follow entirely: `extract_stage` implementations
    /// are free to set `StageFrame::camera_25d` themselves, and the main loop leaves it untouched.
    camera_25d: Option<Camera25D>,
    /// Whether the frame loop profiles (WP6.3, contract §9.2; default `true`).
    profiler: bool,
    /// Budgets per profiler scope (default [`ProfilerBudgets::default`]).
    profiler_budgets: ProfilerBudgets,
}

impl fmt::Debug for AppBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let plugins: Vec<&str> = self.plugins.iter().map(|plugin| plugin.name()).collect();
        f.debug_struct("AppBuilder")
            .field("window", &self.window)
            .field("seed", &self.seed)
            .field("tick_rate_hz", &self.tick_rate_hz)
            .field("max_ticks_per_frame", &self.max_ticks_per_frame)
            .field("hash_every", &self.hash_every)
            .field("input_map", &self.input_map)
            .field("renderer_config", &self.renderer_config)
            .field("plugins", &plugins)
            .field("max_frames", &self.max_frames)
            .field("exit_key", &self.exit_key)
            .field("overlay_key", &self.overlay_key)
            .field("camera_25d", &self.camera_25d)
            .field("profiler", &self.profiler)
            .field("profiler_budgets", &self.profiler_budgets)
            .field(
                "executor_threads",
                &self
                    .executor
                    .as_ref()
                    .map_or(1, |executor| executor.threads()),
            )
            .finish()
    }
}

impl AppBuilder {
    /// Seed of the simulation (default 0).
    #[must_use]
    pub fn seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self
    }

    /// Simulation ticks per second (default 60).
    ///
    /// # Panics
    /// If `hz` is 0.
    #[must_use]
    pub fn tick_rate(mut self, hz: u32) -> Self {
        assert!(hz > 0, "tick rate must be at least 1 Hz");
        self.tick_rate_hz = hz;
        self
    }

    /// Most ticks simulated in one frame; excess time is dropped (default 8).
    ///
    /// # Panics
    /// If `max` is 0.
    #[must_use]
    pub fn max_ticks_per_frame(mut self, max: u32) -> Self {
        assert!(max > 0, "at least one tick per frame must be allowed");
        self.max_ticks_per_frame = max;
        self
    }

    /// Ticks between recorded state hashes in headless reports (default 60, 0 = final only).
    #[must_use]
    pub fn hash_every(mut self, ticks: u64) -> Self {
        self.hash_every = ticks;
        self
    }

    /// Bindings sampled into input slot 0 once per frame and fed to every tick of that frame
    /// (default [`InputMap::default`]).
    #[must_use]
    pub fn input_map(mut self, input_map: InputMap) -> Self {
        self.input_map = input_map;
        self
    }

    /// Configuration of the window renderer used by [`AppBuilder::run`] (and of the offscreen
    /// renderer of [`AppBuilder::run_offscreen`]). Sets only the `base` of
    /// [`AppBuilder::stage_renderer_config`]; light budget and multisampling keep their values.
    #[must_use]
    pub fn renderer_config(mut self, config: RendererConfig) -> Self {
        self.renderer_config.base = config;
        self
    }

    /// Full stage renderer configuration for [`AppBuilder::run`] and
    /// [`AppBuilder::run_offscreen`]: the [`RendererConfig`] plus the point-light budget and
    /// multisampling (default [`StageRendererConfig::default`], contract §6). Replaces an earlier
    /// [`AppBuilder::renderer_config`].
    #[must_use]
    pub fn stage_renderer_config(mut self, config: StageRendererConfig) -> Self {
        self.renderer_config = config;
        self
    }

    /// Adds a plugin. Plugins are called in the order they were added.
    #[must_use]
    pub fn plugin(mut self, plugin: impl GamePlugin + 'static) -> Self {
        self.plugins.push(Box::new(plugin));
        self
    }

    /// Ends the frame loop after `frames` frames (`0` exits right after init).
    #[must_use]
    pub fn max_frames(mut self, frames: u64) -> Self {
        self.max_frames = Some(frames);
        self
    }

    /// Ends the frame loop when `key` is pressed.
    #[must_use]
    pub fn exit_key(mut self, key: KeyCode) -> Self {
        self.exit_key = Some(key);
        self
    }

    /// Key whose press toggles the stats overlay in every frame loop (contract §9.2, plan 0002
    /// WP6.4; default `Some(KeyCode::F3)`, `None` disables toggling). Available in every build.
    ///
    /// The overlay draws the frame profile's scopes with budget bars, red over budget
    /// ([`crate::adapters::debug::StatsOverlay`]), into `StageFrame::debug_sprites` after every
    /// plugin's `extract_stage`. Without the profiler ([`AppBuilder::profiler`]) it shows only
    /// frames per second and frame time. The key still reaches the input state like any other.
    #[must_use]
    pub fn overlay_key(mut self, key: Option<KeyCode>) -> Self {
        self.overlay_key = key;
        self
    }

    /// Enables the render-side camera follow (plan 0002 WP2.4): a template [`Camera25D`] whose
    /// `target` the main loop overwrites every frame with the output of a critically damped
    /// spring plus look-ahead ([`grimoire_render::CameraFollow`]), driven by the first plugin
    /// whose [`GamePlugin::focus`] returns `Some` (default: disabled, `None`).
    ///
    /// Every other field of `camera` (`tilt_degrees`, `fov_y_degrees`, `distance`,
    /// `look_ahead_max`, `look_ahead_smoothing`) is used as configured and never touched by the
    /// loop; only `target` is replaced. While no plugin has produced a focus point yet (for
    /// example before the `fixtures` feature's player proxy has run its first tick), the stage's
    /// camera keeps `camera`'s own `target` unchanged.
    ///
    /// Without this call, `extract_stage` implementations remain free to set
    /// `StageFrame::camera_25d` themselves (or not at all); the loop never overwrites it.
    #[must_use]
    pub fn camera25d(mut self, camera: Camera25D) -> Self {
        self.camera_25d = Some(camera);
        self
    }

    /// Whether the frame loops of [`AppBuilder::run`] and [`AppBuilder::run_headless_frames`]
    /// profile every frame (default `true`, contract §9.2, plan 0002 WP6.3, PRD-0002 FR-12:
    /// available in every build).
    ///
    /// When on, the loop measures the scopes of [`crate::adapters::debug::LOOP_SCOPES`] with the
    /// platform clock, steps the simulation through the profiler's schedule observer (which
    /// records every system under its subsystem scope and never changes a state hash) and calls
    /// [`GamePlugin::on_profile`] after [`GamePlugin::on_frame`]. When off, the loop uses plain
    /// `Simulation::step` and never calls `on_profile`. [`AppBuilder::run_headless`] has no clock
    /// and never profiles.
    #[must_use]
    pub fn profiler(mut self, enabled: bool) -> Self {
        self.profiler = enabled;
        self
    }

    /// Budgets per profiler scope (default [`ProfilerBudgets::default`], the PRD-0002/PRD-0004
    /// budgets of [`crate::adapters::debug::DEFAULT_BUDGETS`]). A scope over its budget reports
    /// [`grimoire_debug::ScopeTotal::over_budget`].
    #[must_use]
    pub fn profiler_budgets(mut self, budgets: ProfilerBudgets) -> Self {
        self.profiler_budgets = budgets;
        self
    }

    /// Executor for parallel stages and data-parallel queries (default: the world's
    /// [`grimoire_ecs::SequentialExecutor`]).
    ///
    /// It is set on the simulation's world right after `Simulation::new`, before any plugin's
    /// `build`, in [`AppBuilder::run`], [`AppBuilder::run_headless`] and the headless frame
    /// loops. The facade creates no threads itself: a game that simulates on several threads
    /// depends on `grimoire_exec` and passes, for example,
    /// `Arc::new(grimoire_exec::ThreadPoolExecutor::new(4)?)`. State hashes do not depend on the
    /// executor (engine ADR-0006).
    #[must_use]
    pub fn executor(mut self, executor: Arc<dyn Executor>) -> Self {
        self.executor = Some(executor);
        self
    }

    /// Opens the window and runs the main loop until the window closes, the exit key is pressed,
    /// the frame limit is reached or rendering fails.
    ///
    /// Must be called on the main thread (macOS requirement of the window and GPU layers).
    ///
    /// On macOS, quitting through the application menu (Cmd+Q) ends the process inside the
    /// platform layer after [`GamePlugin::shutdown`] ran for every plugin: this method does not
    /// return and destructors do not run. Work that must happen at the end of a run belongs in
    /// [`GamePlugin::shutdown`].
    ///
    /// # Errors
    /// - [`GrimoireError::Platform`] if the event loop or the window cannot be created.
    /// - [`GrimoireError::Render`] if the renderer cannot be created or rendering fails with
    ///   anything other than [`grimoire_render::RenderError::SurfaceLost`].
    pub fn run(self) -> Result<(), GrimoireError> {
        let renderer_config = self.renderer_config.clone();
        let factory: RendererFactory<WgpuRenderer> = Box::new(move |ctx| {
            let window = ctx.window().ok_or_else(|| {
                PlatformError::WindowCreation(String::from("the runner provided no window"))
            })?;
            Ok(WgpuRenderer::new_for_window_staged(
                window,
                renderer_config,
            )?)
        });
        let window = self.window.clone();
        let outcome = Outcome::default();
        let game_loop = self.into_loop(factory, false, Rc::clone(&outcome));
        let result = run_desktop(window, game_loop);
        if let Some(error) = outcome.borrow_mut().take() {
            return Err(error);
        }
        result.map_err(GrimoireError::from)
    }

    /// Runs only the simulation for `ticks` ticks — no platform, no renderer, no frame loop.
    ///
    /// Plugins are built once in registration order; `extract` and `on_frame` are never called.
    /// `input(tick)` supplies the input of the tick about to be simulated.
    #[must_use]
    pub fn run_headless(
        mut self,
        ticks: u64,
        input: &mut dyn FnMut(u64) -> TickInput,
    ) -> HeadlessReport {
        let mut sim = Simulation::new(self.seed);
        if let Some(executor) = &self.executor {
            sim.world_mut().set_executor(Arc::clone(executor));
        }
        for plugin in &mut self.plugins {
            plugin.build(&mut sim);
        }
        let mut hashes = Vec::new();
        for _ in 0..ticks {
            sim.step(input(sim.tick()));
            if self.hash_every != 0 && sim.tick().is_multiple_of(self.hash_every) {
                hashes.push((sim.tick(), sim.state_hash()));
            }
        }
        let final_tick = sim.tick();
        let final_hash = sim.state_hash();
        if hashes.last().map(|&(tick, _)| tick) != Some(final_tick) {
            hashes.push((final_tick, final_hash));
        }
        HeadlessReport {
            final_tick,
            final_hash,
            hashes,
        }
    }

    /// Drives the real main loop for `frames` frames without a window, with a
    /// [`NullRenderer`] and a manual clock advancing by `frame_delta` before every frame.
    ///
    /// No platform events occur; see [`AppBuilder::run_headless_frames_with_events`].
    ///
    /// # Errors
    /// Like [`AppBuilder::run`]; the null renderer itself never fails.
    pub fn run_headless_frames(
        self,
        frames: u64,
        frame_delta: Duration,
    ) -> Result<LoopReport, GrimoireError> {
        self.run_headless_frames_with_events(frames, frame_delta, &mut |_, _| {})
    }

    /// Like [`AppBuilder::run_headless_frames`], but before frame `n` (0-based) the events that
    /// `events(n, &mut buffer)` pushes are delivered to the loop, in order.
    ///
    /// # Errors
    /// Like [`AppBuilder::run`].
    pub fn run_headless_frames_with_events(
        self,
        frames: u64,
        frame_delta: Duration,
        events: &mut dyn FnMut(u64, &mut Vec<PlatformEvent>),
    ) -> Result<LoopReport, GrimoireError> {
        let factory: RendererFactory<NullRenderer> = Box::new(|_| Ok(NullRenderer::default()));
        let outcome = Outcome::default();
        let mut game_loop = self.into_loop(factory, true, Rc::clone(&outcome));
        let mut scripted = ScriptedEvents {
            inner: &mut game_loop,
            script: events,
            events: Vec::new(),
            frame: 0,
        };
        let result = run_headless(&mut scripted, frames, frame_delta);
        if let Some(error) = outcome.borrow_mut().take() {
            return Err(error);
        }
        result?;
        game_loop.report().ok_or_else(|| {
            GrimoireError::Platform(PlatformError::AppInit(String::from(
                "the main loop was not initialised",
            )))
        })
    }

    /// Drives the real main loop for `run.frames` frames into an offscreen image of
    /// `run.width` × `run.height` pixels, with the same `WgpuRenderer` configuration
    /// [`AppBuilder::run`] uses ([`AppBuilder::stage_renderer_config`]) and a manual clock advancing
    /// by `run.frame_delta` before every frame. No window exists: `window_created` is never called.
    ///
    /// Plugins register their assets with that offscreen renderer
    /// ([`GamePlugin::register_assets`]), so a test or a capture tool sees exactly what the desktop
    /// run draws. Before frame `n` (0-based) the events `events(n, &mut buffer)` pushes are
    /// delivered, as in [`AppBuilder::run_headless_frames_with_events`]; mouse-aim sampling sees a
    /// viewport of the offscreen size. After every completed frame `n` with
    /// `n % run.capture_every == 0` (never for `capture_every == 0`), `capture(n, rgba)` receives
    /// the rendered image as tightly packed sRGB RGBA8 rows, top row first.
    ///
    /// Takes the GPU like [`AppBuilder::run`]: tests set `GRIMOIRE_GPU_ADAPTER=software`.
    ///
    /// # Errors
    /// - [`GrimoireError::Render`] if the offscreen renderer cannot be created (for example
    ///   [`grimoire_render::RenderError::NoAdapter`] or a zero size), rendering fails or an image
    ///   cannot be read back.
    /// - [`GrimoireError::Assets`] if a plugin's asset registration fails.
    pub fn run_offscreen(
        mut self,
        run: OffscreenRun,
        events: &mut dyn FnMut(u64, &mut Vec<PlatformEvent>),
        capture: &mut dyn FnMut(u64, &[u8]),
    ) -> Result<LoopReport, GrimoireError> {
        let renderer_config = self.renderer_config.clone();
        let (width, height) = (run.width, run.height);
        let factory: RendererFactory<WgpuRenderer> = Box::new(move |_| {
            Ok(WgpuRenderer::new_offscreen_staged(
                width,
                height,
                renderer_config,
            )?)
        });
        // Mouse-aim sampling sees the offscreen image as its viewport (contract §9.3).
        self.window.width = width;
        self.window.height = height;
        let outcome = Outcome::default();
        let mut game_loop = self.into_loop(factory, true, Rc::clone(&outcome));
        let mut offscreen = OffscreenFrames {
            scripted: ScriptedEvents {
                inner: &mut game_loop,
                script: events,
                events: Vec::new(),
                frame: 0,
            },
            capture_every: run.capture_every,
            capture,
        };
        let result = run_headless(&mut offscreen, run.frames, run.frame_delta);
        if let Some(error) = outcome.borrow_mut().take() {
            return Err(error);
        }
        result?;
        game_loop.report().ok_or_else(|| {
            GrimoireError::Platform(PlatformError::AppInit(String::from(
                "the main loop was not initialised",
            )))
        })
    }

    fn into_loop<R: LoopRenderer>(
        self,
        factory: RendererFactory<R>,
        record_hashes: bool,
        outcome: Outcome,
    ) -> GameLoop<R> {
        let settings = LoopSettings {
            seed: self.seed,
            tick_rate_hz: self.tick_rate_hz,
            max_ticks_per_frame: self.max_ticks_per_frame,
            hash_every: self.hash_every,
            input_map: self.input_map,
            max_frames: self.max_frames,
            exit_key: self.exit_key,
            overlay_key: self.overlay_key,
            record_hashes,
            executor: self.executor,
            camera_25d: self.camera_25d,
            profiler: self.profiler.then_some(self.profiler_budgets),
            // Physical pixels; a real `Resized` event overwrites this once the window exists
            // (contract §9.3), but `run_headless_frames*` never resizes, so the configured
            // `WindowConfig` size is what mouse-aim sampling sees throughout those runs.
            initial_viewport: (self.window.width as f32, self.window.height as f32),
        };
        GameLoop::new(settings, self.plugins, factory, outcome)
    }
}
