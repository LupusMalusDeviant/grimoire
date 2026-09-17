//! The main loop shared by the desktop run and the headless frame loop.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use grimoire_core::Vec2;
use grimoire_ecs::Executor;
use grimoire_platform::{
    AppHandler, AppResult, KeyCode, PlatformContext, PlatformEvent, RawInputEvent,
};
use grimoire_render::{Camera25D, CameraFollow, RenderError, StageFrame, StageStats};
use grimoire_sim::{FixedTimestep, Simulation, TickInput};

use crate::adapters::debug::{
    Profiler, ProfilerBudgets, SCOPE_EXTRACT, SCOPE_FRAME, SCOPE_RENDER, SCOPE_SIM, StatsOverlay,
};
use crate::aim::{PointerState, sample_aim};
use crate::error::GrimoireError;
use crate::input::{InputMap, InputState};
use crate::plugin::{FrameStats, GamePlugin};
use crate::render_assets::{HeadlessRenderAssets, LoopRenderer};
use grimoire_render::WgpuRenderer;

/// Length of the window over which [`FrameStats::fps`] is averaged.
const FPS_WINDOW: Duration = Duration::from_secs(1);

/// Creates the renderer during `init`, when the window (if any) exists.
pub(crate) type RendererFactory<R> =
    Box<dyn FnOnce(&mut dyn PlatformContext) -> Result<R, GrimoireError>>;

/// Error slot that outlives the loop: `run_desktop` consumes the app it drives.
pub(crate) type Outcome = Rc<RefCell<Option<GrimoireError>>>;

/// Configuration of one run, taken from the builder.
pub(crate) struct LoopSettings {
    pub seed: u64,
    pub tick_rate_hz: u32,
    pub max_ticks_per_frame: u32,
    pub hash_every: u64,
    pub input_map: InputMap,
    pub max_frames: Option<u64>,
    pub exit_key: Option<KeyCode>,
    /// Key that toggles the stats overlay (WP6.4, contract §9.2); `None` never toggles it.
    pub overlay_key: Option<KeyCode>,
    pub record_hashes: bool,
    /// Set on the world right after `Simulation::new`; `None` keeps the sequential default.
    pub executor: Option<Arc<dyn Executor>>,
    /// `None` disables the WP2.4 render-side camera follow (see [`crate::AppBuilder::camera25d`]).
    pub camera_25d: Option<Camera25D>,
    /// Viewport size (physical pixels) mouse-aim sampling uses before the first
    /// [`PlatformEvent::Resized`] (contract §9.3): `WindowConfig::width`/`height`.
    pub initial_viewport: (f32, f32),
    /// Budgets of the frame profiler (WP6.3), `None` when [`crate::AppBuilder::profiler`] is off.
    pub profiler: Option<ProfilerBudgets>,
}

/// Result of [`crate::AppBuilder::run_headless_frames`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopReport {
    /// Frames completed (extracted, rendered and reported to `on_frame`).
    pub frames: u64,
    /// Simulation tick count at the end of the run.
    pub final_tick: u64,
    /// [`Simulation::state_hash`] at the end of the run.
    pub final_hash: u64,
    /// `(tick, state_hash)` after every tick that is a multiple of `hash_every`, then the final
    /// state unless it was just recorded (the semantics of [`grimoire_sim::replay`]).
    pub hashes: Vec<(u64, u64)>,
    /// Real time the fixed timestep discarded because frames exceeded the tick limit.
    pub dropped_time: Duration,
}

struct Running<R> {
    sim: Simulation,
    renderer: R,
}

/// Measures frames per second over windows of at least [`FPS_WINDOW`].
struct FpsCounter {
    window_start: Duration,
    frames: u64,
    fps: f64,
}

impl FpsCounter {
    fn new(now: Duration) -> Self {
        Self {
            window_start: now,
            frames: 0,
            fps: 0.0,
        }
    }

    fn frame(&mut self, now: Duration) -> f64 {
        self.frames += 1;
        let elapsed = now.saturating_sub(self.window_start);
        if elapsed >= FPS_WINDOW {
            self.fps = self.frames as f64 / elapsed.as_secs_f64();
            self.frames = 0;
            self.window_start = now;
        }
        self.fps
    }
}

/// Fixed-timestep main loop over any [`Renderer`], driven by a platform runner.
pub(crate) struct GameLoop<R: LoopRenderer> {
    settings: LoopSettings,
    plugins: Vec<Box<dyn GamePlugin>>,
    factory: Option<RendererFactory<R>>,
    running: Option<Running<R>>,
    input: InputState,
    /// Last known cursor position (contract §9.2/§9.3).
    pointer: PointerState,
    /// Viewport size in physical pixels, updated by [`PlatformEvent::Resized`] (contract §9.3).
    viewport: (f32, f32),
    stage: StageFrame,
    /// Render-side camera follow spring (plan 0002 WP2.4), created lazily once a plugin first
    /// produces a focus point; `None` forever when [`LoopSettings::camera_25d`] is `None`.
    camera_follow: Option<CameraFollow>,
    /// Focus point and camera of the most recently rendered [`StageFrame`], held for the next
    /// frame's mouse-aim sampling (contract §9.3 step 5, "bis zur Abtastung des nächsten Frames
    /// gehalten").
    held_focus: Option<Vec2>,
    held_camera: Option<Camera25D>,
    /// Whether the one-time "renderer has no `render_stage` override" log line has run yet
    /// (contract §9.3: "meldet die Fassade das einmal im Log").
    logged_missing_stage_support: bool,
    /// Frame profiler (plan 0002 WP6.3, contract §9.7); `None` when turned off.
    profiler: Option<Profiler>,
    /// Stats overlay (plan 0002 WP6.4, contract §9.7); presentation state only.
    overlay: StatsOverlay,
    timestep: FixedTimestep,
    last_time: Duration,
    fps: FpsCounter,
    frames: u64,
    hashes: Vec<(u64, u64)>,
    outcome: Outcome,
}

impl<R: LoopRenderer> GameLoop<R> {
    pub(crate) fn new(
        settings: LoopSettings,
        plugins: Vec<Box<dyn GamePlugin>>,
        factory: RendererFactory<R>,
        outcome: Outcome,
    ) -> Self {
        let timestep = FixedTimestep::new(settings.tick_rate_hz)
            .with_max_ticks_per_frame(settings.max_ticks_per_frame);
        let viewport = settings.initial_viewport;
        let profiler = settings.profiler.clone().map(Profiler::new);
        Self {
            settings,
            plugins,
            factory: Some(factory),
            running: None,
            input: InputState::new(),
            pointer: PointerState::new(),
            viewport,
            stage: StageFrame::new(),
            camera_follow: None,
            held_focus: None,
            held_camera: None,
            logged_missing_stage_support: false,
            profiler,
            overlay: StatsOverlay::new(),
            timestep,
            last_time: Duration::ZERO,
            fps: FpsCounter::new(Duration::ZERO),
            frames: 0,
            hashes: Vec::new(),
            outcome,
        }
    }

    /// Report of the run so far, or `None` if `init` never succeeded.
    pub(crate) fn report(&self) -> Option<LoopReport> {
        let running = self.running.as_ref()?;
        let final_tick = running.sim.tick();
        let final_hash = running.sim.state_hash();
        let mut hashes = self.hashes.clone();
        if hashes.last().map(|&(tick, _)| tick) != Some(final_tick) {
            hashes.push((final_tick, final_hash));
        }
        Some(LoopReport {
            frames: self.frames,
            final_tick,
            final_hash,
            hashes,
            dropped_time: self.timestep.dropped_time(),
        })
    }

    fn fail(&self, error: GrimoireError) {
        self.outcome.borrow_mut().get_or_insert(error);
    }
}

impl<R: LoopRenderer> AppHandler for GameLoop<R> {
    fn init(&mut self, ctx: &mut dyn PlatformContext) -> AppResult {
        let factory = self
            .factory
            .take()
            .ok_or("the main loop was initialised twice")?;
        let renderer = match factory(ctx) {
            Ok(renderer) => renderer,
            Err(error) => {
                let message = error.to_string();
                self.fail(error);
                return Err(message.into());
            }
        };
        log::info!("renderer backend: {}", renderer.backend_name());

        let mut sim = Simulation::new(self.settings.seed);
        if let Some(executor) = &self.settings.executor {
            sim.world_mut().set_executor(Arc::clone(executor));
        }
        for plugin in &mut self.plugins {
            log::debug!("building plugin {}", plugin.name());
            plugin.build(&mut sim);
        }

        // Contract §9.2 asset hook: every plugin registers its meshes and textures with this
        // loop's renderer before the first frame. A failure ends the run like a render error, but
        // after a successful `init`, so every built plugin still receives `shutdown`.
        let mut renderer = renderer;
        let mut headless_assets = HeadlessRenderAssets::new();
        let mut assets_failed = false;
        for plugin in &mut self.plugins {
            let assets = renderer.render_assets(&mut headless_assets);
            if let Err(source) = plugin.register_assets(assets) {
                let plugin = plugin.name().to_owned();
                log::error!(
                    "plugin {plugin} failed to register its assets, ending the run: {source}"
                );
                self.fail(GrimoireError::Assets { plugin, source });
                assets_failed = true;
                break;
            }
        }

        if !assets_failed && let Some(window) = ctx.window() {
            for plugin in &mut self.plugins {
                plugin.window_created(&window);
            }
        }

        self.last_time = ctx.clock().elapsed();
        self.fps = FpsCounter::new(self.last_time);
        self.running = Some(Running { sim, renderer });
        if assets_failed || self.settings.max_frames == Some(0) {
            ctx.request_exit();
        }
        Ok(())
    }

    fn event(&mut self, ctx: &mut dyn PlatformContext, event: &PlatformEvent) {
        match event {
            PlatformEvent::Resized(size) => {
                // Physical-pixel viewport for mouse-aim sampling (contract §9.3); tracked even
                // before `init`/without a renderer, like `WindowConfig::width`/`height` before the
                // first resize.
                self.viewport = (size.width as f32, size.height as f32);
                if let Some(running) = &mut self.running {
                    running.renderer.resize(size.width, size.height);
                }
            }
            // Release events for keys held while unfocused never arrive (PRD-0013 robustness).
            PlatformEvent::Focused(false) => self.input.release_all(),
            PlatformEvent::Input(raw) => {
                if let RawInputEvent::Key {
                    code,
                    pressed: true,
                    repeat: false,
                } = *raw
                {
                    if self.settings.exit_key == Some(code) {
                        ctx.request_exit();
                    }
                    // Contract §9.3: a press toggles the overlay. The key still reaches
                    // `InputState` like any other, so a game that binds it sees it too.
                    if self.settings.overlay_key == Some(code) {
                        self.overlay.toggle();
                    }
                }
                self.input.apply(raw);
                self.pointer.apply(raw);
            }
            PlatformEvent::Focused(true)
            | PlatformEvent::Occluded(_)
            | PlatformEvent::ScaleFactorChanged(_)
            | PlatformEvent::CloseRequested => {}
        }
    }

    fn frame(&mut self, ctx: &mut dyn PlatformContext) {
        let Some(running) = self.running.as_mut() else {
            return;
        };

        // The only wall-clock read of the run; the simulation only sees whole ticks.
        let now = ctx.clock().elapsed();
        let frame_time = now.saturating_sub(self.last_time);
        self.last_time = now;
        let plan = self.timestep.advance(frame_time);

        let mut tick_input = TickInput::default();
        tick_input.slots[0] = self.settings.input_map.sample(&self.input);
        // Contract §9.3 step 3 / §9.4: the camera and focus point held from the previously
        // rendered frame, sampled once per frame and applied to every tick due this frame. Never
        // reaches the simulation by any other path, so which camera (if any) produced these axes
        // cannot change a state hash (plan 0002 WP2.4 replay gate) — only the recorded `i16`
        // values do.
        if let Some(camera) = &self.held_camera
            && let Some(focus) = self.held_focus
            && let Some(cursor) = self.pointer.position()
            && let Some(axes) =
                sample_aim(camera, cursor, [self.viewport.0, self.viewport.1], focus)
        {
            tick_input.slots[0].axes[2] = axes[0];
            tick_input.slots[0].axes[3] = axes[1];
        }

        if let Some(profiler) = &mut self.profiler {
            profiler.begin_frame(self.frames);
        }
        // Contract §9.3 step 4 / §9.7: with the profiler, every tick runs through its schedule
        // observer (read-only, no hash changes) and `sim` sums the steps.
        let mut sim_time = Duration::ZERO;
        for _ in 0..plan.ticks {
            match &mut self.profiler {
                Some(profiler) => {
                    let clock = ctx.clock();
                    let started = clock.elapsed();
                    running
                        .sim
                        .step_observed(tick_input, &mut profiler.observer(clock));
                    sim_time += clock.elapsed().saturating_sub(started);
                }
                None => running.sim.step(tick_input),
            }
            let tick = running.sim.tick();
            if self.settings.record_hashes
                && self.settings.hash_every != 0
                && tick.is_multiple_of(self.settings.hash_every)
            {
                self.hashes.push((tick, running.sim.state_hash()));
            }
        }
        // A frame without ticks keeps the latch, so a tap released before the next tick survives.
        if plan.ticks > 0 {
            self.input.clear_presses();
        }
        if let Some(profiler) = &mut self.profiler {
            profiler.record(SCOPE_SIM, sim_time);
        }

        let extract_started = self.profiler.as_ref().map(|_| ctx.clock().elapsed());
        self.stage.clear();
        for plugin in &mut self.plugins {
            plugin.extract(running.sim.world(), plan.alpha, &mut self.stage.base);
        }
        for plugin in &mut self.plugins {
            plugin.extract_stage(running.sim.world(), plan.alpha, &mut self.stage);
        }
        if let (Some(profiler), Some(started)) = (&mut self.profiler, extract_started) {
            profiler.record(SCOPE_EXTRACT, ctx.clock().elapsed().saturating_sub(started));
        }

        // Contract §9.3 step 5: the first plugin with an opinion wins and the rest are not asked;
        // presentation-only, like `extract`/`extract_stage` themselves.
        let focus = self
            .plugins
            .iter()
            .find_map(|plugin| plugin.focus(running.sim.world(), plan.alpha));
        if let Some(template) = self.settings.camera_25d
            && let Some(focus) = focus
        {
            let follow = self
                .camera_follow
                .get_or_insert_with(|| CameraFollow::new(focus.to_array()));
            let target = follow.update(&template, focus.to_array(), frame_time.as_secs_f32());
            // `Camera25D` is `#[non_exhaustive]`: field assignment onto the (already owned, since
            // `Camera25D: Copy`) template, not struct-literal update syntax (contract §2 rule 13).
            let mut camera = template;
            camera.target = target;
            self.stage.camera_25d = Some(camera);
        }
        // Held until the next frame's aim sampling above (contract §9.3: "Fokuspunkt und
        // gerenderte Kamera werden bis zur Abtastung des nächsten Frames gehalten"). Whatever
        // ended up in `stage.camera_25d` counts, whether the follow spring above set it or a
        // plugin's own `extract_stage` did (when `camera_25d` is disabled).
        self.held_focus = focus;
        self.held_camera = self.stage.camera_25d;

        // Contract §9.3 step 5 / §9.7: the overlay goes into the debug channel after every
        // `extract_stage`, showing the profile of the frames recorded so far.
        self.overlay.draw(
            &self.stage.base.camera,
            [self.viewport.0, self.viewport.1],
            &mut self.stage.debug_sprites,
        );

        if !self.logged_missing_stage_support && !running.renderer.supports_stage() {
            log::info!(
                "renderer {} has no render_stage override: only StageFrame::base is drawn",
                running.renderer.backend_name()
            );
            self.logged_missing_stage_support = true;
        }

        let render_started = self.profiler.as_ref().map(|_| ctx.clock().elapsed());
        let render = match running.renderer.render_stage(&self.stage) {
            Ok(stats) => stats,
            Err(RenderError::SurfaceLost) => {
                ctx.frame_not_presented();
                StageStats::default()
            }
            Err(error) => {
                log::error!("rendering failed, ending the run: {error}");
                self.fail(error.into());
                ctx.request_exit();
                return;
            }
        };

        if let (Some(profiler), Some(started)) = (&mut self.profiler, render_started) {
            let render_time = ctx.clock().elapsed().saturating_sub(started);
            profiler.record(SCOPE_RENDER, render_time);
            profiler.record_stage_stats(&render, render_time);
        }

        let stats = FrameStats {
            frame: self.frames,
            sim_tick: running.sim.tick(),
            ticks_this_frame: plan.ticks,
            alpha: plan.alpha,
            frame_time,
            fps: self.fps.frame(now),
            dropped_time: self.timestep.dropped_time(),
            render: render.base,
        };
        for plugin in &mut self.plugins {
            plugin.on_frame(&stats);
        }
        if let Some(profiler) = &mut self.profiler {
            profiler.record(SCOPE_FRAME, ctx.clock().elapsed().saturating_sub(now));
            for plugin in &mut self.plugins {
                plugin.on_profile(profiler.profile());
            }
        }
        self.overlay
            .record(&stats, self.profiler.as_ref().map(Profiler::profile));
        self.frames += 1;
        if self
            .settings
            .max_frames
            .is_some_and(|max| self.frames >= max)
        {
            ctx.request_exit();
        }
    }

    fn shutdown(&mut self) {
        // Plugins are built only after the renderer exists, together with `running`.
        if self.running.is_none() {
            return;
        }
        for plugin in &mut self.plugins {
            log::debug!("shutting down plugin {}", plugin.name());
            plugin.shutdown();
        }
    }
}

/// Feeds scripted platform events to a loop before each frame, for headless runs.
pub(crate) struct ScriptedEvents<'a, R: LoopRenderer> {
    pub inner: &'a mut GameLoop<R>,
    pub script: &'a mut dyn FnMut(u64, &mut Vec<PlatformEvent>),
    pub events: Vec<PlatformEvent>,
    pub frame: u64,
}

impl<R: LoopRenderer> AppHandler for ScriptedEvents<'_, R> {
    fn init(&mut self, ctx: &mut dyn PlatformContext) -> AppResult {
        self.inner.init(ctx)
    }

    fn event(&mut self, ctx: &mut dyn PlatformContext, event: &PlatformEvent) {
        self.inner.event(ctx, event);
    }

    fn frame(&mut self, ctx: &mut dyn PlatformContext) {
        self.events.clear();
        (self.script)(self.frame, &mut self.events);
        self.frame += 1;
        for event in &self.events {
            self.inner.event(ctx, event);
            // Like the desktop runner: nothing follows a requested exit.
            if ctx.exit_requested() {
                return;
            }
        }
        self.inner.frame(ctx);
    }

    fn shutdown(&mut self) {
        self.inner.shutdown();
    }
}

/// Drives a scripted loop over an offscreen `WgpuRenderer` and hands selected frames' images to a
/// capture callback ([`crate::AppBuilder::run_offscreen`]).
pub(crate) struct OffscreenFrames<'a> {
    pub scripted: ScriptedEvents<'a, WgpuRenderer>,
    /// `0` never captures.
    pub capture_every: u64,
    pub capture: &'a mut dyn FnMut(u64, &[u8]),
}

impl AppHandler for OffscreenFrames<'_> {
    fn init(&mut self, ctx: &mut dyn PlatformContext) -> AppResult {
        self.scripted.init(ctx)
    }

    fn event(&mut self, ctx: &mut dyn PlatformContext, event: &PlatformEvent) {
        self.scripted.event(ctx, event);
    }

    fn frame(&mut self, ctx: &mut dyn PlatformContext) {
        let frame = self.scripted.inner.frames;
        self.scripted.frame(ctx);
        let completed = self.scripted.inner.frames > frame;
        if !completed || self.capture_every == 0 || !frame.is_multiple_of(self.capture_every) {
            return;
        }
        let game_loop = &mut *self.scripted.inner;
        let Some(running) = game_loop.running.as_mut() else {
            return;
        };
        match running.renderer.read_offscreen_rgba() {
            Ok(rgba) => (self.capture)(frame, &rgba),
            Err(error) => {
                log::error!("reading back offscreen frame {frame} failed, ending the run: {error}");
                game_loop.fail(error.into());
                ctx.request_exit();
            }
        }
    }

    fn shutdown(&mut self) {
        self.scripted.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use grimoire_platform::{Clock, ManualClock, PhysicalSize, PlatformWindow, run_headless};
    use grimoire_render::{RenderFrame, RenderStats, Renderer};

    use crate::render_assets::RenderAssets;

    /// Fails with the scripted error on the given render call (0-based) and records resizes.
    struct ScriptedRenderer {
        calls: u64,
        fail_on: u64,
        error: fn() -> RenderError,
        /// `(render calls before the resize, width, height)`.
        resizes: Vec<(u64, u32, u32)>,
    }

    impl ScriptedRenderer {
        fn new(fail_on: u64, error: fn() -> RenderError) -> Self {
            Self {
                calls: 0,
                fail_on,
                error,
                resizes: Vec::new(),
            }
        }
    }

    impl Renderer for ScriptedRenderer {
        fn resize(&mut self, width: u32, height: u32) {
            self.resizes.push((self.calls, width, height));
        }

        fn render(&mut self, _frame: &RenderFrame) -> Result<RenderStats, RenderError> {
            let call = self.calls;
            self.calls += 1;
            if call == self.fail_on {
                return Err((self.error)());
            }
            Ok(RenderStats {
                draw_calls: 1,
                ..RenderStats::default()
            })
        }

        fn backend_name(&self) -> &str {
            "Scripted"
        }
    }

    impl LoopRenderer for ScriptedRenderer {
        fn render_assets<'a>(
            &'a mut self,
            headless: &'a mut HeadlessRenderAssets,
        ) -> &'a mut dyn RenderAssets {
            headless
        }
    }

    #[derive(Default)]
    struct Frames(Rc<RefCell<Vec<FrameStats>>>);

    impl GamePlugin for Frames {
        fn name(&self) -> &str {
            "frames"
        }

        fn on_frame(&mut self, stats: &FrameStats) {
            self.0.borrow_mut().push(*stats);
        }
    }

    fn settings() -> LoopSettings {
        LoopSettings {
            seed: 1,
            tick_rate_hz: 60,
            max_ticks_per_frame: 8,
            hash_every: 0,
            input_map: InputMap::default(),
            max_frames: None,
            exit_key: None,
            overlay_key: None,
            record_hashes: true,
            executor: None,
            camera_25d: None,
            initial_viewport: (800.0, 600.0),
            profiler: Some(ProfilerBudgets::default()),
        }
    }

    fn run_scripted(
        fail_on: u64,
        error: fn() -> RenderError,
    ) -> (Outcome, Vec<FrameStats>, Option<LoopReport>, (u32, u32)) {
        let outcome = Outcome::default();
        let frames = Frames::default();
        let seen = Rc::clone(&frames.0);
        let lifecycle = Lifecycle::default();
        let counts = Rc::clone(&lifecycle.0);
        let factory: RendererFactory<ScriptedRenderer> =
            Box::new(move |_| Ok(ScriptedRenderer::new(fail_on, error)));
        let mut game_loop = GameLoop::new(
            settings(),
            vec![Box::new(frames), Box::new(lifecycle)],
            factory,
            Rc::clone(&outcome),
        );
        run_headless(&mut game_loop, 10, Duration::from_millis(16)).expect("init succeeds");
        let report = game_loop.report();
        let seen = seen.borrow().clone();
        let counts = *counts.borrow();
        (outcome, seen, report, counts)
    }

    #[test]
    fn surface_lost_skips_the_frame_and_keeps_running() {
        let (outcome, frames, report, lifecycle) = run_scripted(2, || RenderError::SurfaceLost);
        assert!(outcome.borrow().is_none());
        assert_eq!(lifecycle, (1, 1));
        assert_eq!(frames.len(), 10);
        assert_eq!(frames[2].render, RenderStats::default());
        assert_eq!(frames[3].render.draw_calls, 1);
        assert_eq!(report.map(|report| report.frames), Some(10));
    }

    /// Headless context that records what the loop reports to the runner.
    #[derive(Default)]
    struct RecordingContext {
        clock: ManualClock,
        exit_requested: bool,
        not_presented: Vec<u64>,
        frame: u64,
    }

    impl PlatformContext for RecordingContext {
        fn window(&self) -> Option<std::sync::Arc<dyn PlatformWindow>> {
            None
        }

        fn clock(&self) -> &dyn Clock {
            &self.clock
        }

        fn request_exit(&mut self) {
            self.exit_requested = true;
        }

        fn exit_requested(&self) -> bool {
            self.exit_requested
        }

        fn frame_not_presented(&mut self) {
            self.not_presented.push(self.frame);
        }
    }

    #[test]
    fn surface_lost_is_reported_to_the_runner_as_not_presented() {
        let factory: RendererFactory<ScriptedRenderer> =
            Box::new(|_| Ok(ScriptedRenderer::new(2, || RenderError::SurfaceLost)));
        let mut game_loop = GameLoop::new(settings(), Vec::new(), factory, Outcome::default());
        let mut ctx = RecordingContext::default();
        game_loop.init(&mut ctx).expect("init succeeds");
        for frame in 0..5 {
            ctx.frame = frame;
            ctx.clock.advance(Duration::from_millis(16));
            game_loop.frame(&mut ctx);
        }
        assert_eq!(ctx.not_presented, [2]);
        assert!(!ctx.exit_requested);
    }

    #[test]
    fn resized_events_reach_the_renderer_before_the_next_render() {
        let factory: RendererFactory<ScriptedRenderer> =
            Box::new(|_| Ok(ScriptedRenderer::new(u64::MAX, || RenderError::SurfaceLost)));
        let mut game_loop = GameLoop::new(settings(), Vec::new(), factory, Outcome::default());

        // No renderer exists before init; the event is dropped instead of panicking.
        let mut early = RecordingContext::default();
        game_loop.event(&mut early, &PlatformEvent::Resized(PhysicalSize::new(1, 1)));

        let mut script = |frame: u64, events: &mut Vec<PlatformEvent>| match frame {
            3 => events.push(PlatformEvent::Resized(PhysicalSize::new(800, 600))),
            5 => events.extend([
                PlatformEvent::Resized(PhysicalSize::new(0, 0)),
                PlatformEvent::Resized(PhysicalSize::new(1280, 720)),
            ]),
            _ => {}
        };
        let mut scripted = ScriptedEvents {
            inner: &mut game_loop,
            script: &mut script,
            events: Vec::new(),
            frame: 0,
        };
        run_headless(&mut scripted, 6, Duration::from_millis(16)).expect("init succeeds");

        let renderer = &game_loop.running.as_ref().expect("init ran").renderer;
        assert_eq!(renderer.resizes, [(3, 800, 600), (5, 0, 0), (5, 1280, 720)]);
        assert_eq!(renderer.calls, 6);
    }

    #[test]
    fn other_render_errors_end_the_run_with_the_error() {
        let (outcome, frames, report, lifecycle) = run_scripted(3, || RenderError::OutOfMemory);
        assert!(matches!(
            *outcome.borrow(),
            Some(GrimoireError::Render(RenderError::OutOfMemory))
        ));
        assert_eq!(frames.len(), 3, "on_frame is skipped for the failed frame");
        assert_eq!(report.map(|report| report.frames), Some(3));
        assert_eq!(
            lifecycle,
            (1, 1),
            "plugins still shut down after a render error"
        );
    }

    /// Counts `build` and `shutdown` calls.
    #[derive(Default)]
    struct Lifecycle(Rc<RefCell<(u32, u32)>>);

    impl GamePlugin for Lifecycle {
        fn name(&self) -> &str {
            "lifecycle"
        }

        fn build(&mut self, _sim: &mut Simulation) {
            self.0.borrow_mut().0 += 1;
        }

        fn shutdown(&mut self) {
            self.0.borrow_mut().1 += 1;
        }
    }

    #[test]
    fn renderer_creation_failure_is_kept_and_skips_build_and_shutdown() {
        let outcome = Outcome::default();
        let factory: RendererFactory<ScriptedRenderer> =
            Box::new(|_| Err(RenderError::NoAdapter.into()));
        let plugin = Lifecycle::default();
        let counts = Rc::clone(&plugin.0);
        let mut game_loop = GameLoop::new(
            settings(),
            vec![Box::new(plugin)],
            factory,
            Rc::clone(&outcome),
        );
        let error = run_headless(&mut game_loop, 5, Duration::from_millis(16)).unwrap_err();
        // A runner never calls `shutdown` after a failed `init`; the loop must not act on one.
        game_loop.shutdown();
        assert_eq!(*counts.borrow(), (0, 0), "neither build nor shutdown ran");
        assert!(matches!(
            error,
            grimoire_platform::PlatformError::AppInit(_)
        ));
        assert!(matches!(
            *outcome.borrow(),
            Some(GrimoireError::Render(RenderError::NoAdapter))
        ));
        assert!(game_loop.report().is_none());
    }

    /// A manual clock that additionally moves one microsecond on every reading, so every interval
    /// the loop measures is non-zero and deterministic.
    #[derive(Default)]
    struct SteppingClock {
        base: ManualClock,
        readings: std::sync::atomic::AtomicU64,
    }

    impl Clock for SteppingClock {
        fn elapsed(&self) -> Duration {
            let readings = self
                .readings
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.base.elapsed() + Duration::from_micros(readings)
        }
    }

    #[derive(Default)]
    struct SteppingContext {
        clock: SteppingClock,
        exit_requested: bool,
    }

    impl PlatformContext for SteppingContext {
        fn window(&self) -> Option<std::sync::Arc<dyn PlatformWindow>> {
            None
        }

        fn clock(&self) -> &dyn Clock {
            &self.clock
        }

        fn request_exit(&mut self) {
            self.exit_requested = true;
        }

        fn exit_requested(&self) -> bool {
            self.exit_requested
        }

        fn frame_not_presented(&mut self) {}
    }

    /// `(scope name, total, calls, budget, estimate)` of every scope, plus the frame index.
    type ProfileRows = Vec<(u64, Vec<(String, Duration, u32, Option<Duration>, bool)>)>;

    #[derive(Default)]
    struct ProfileProbe {
        rows: Rc<RefCell<ProfileRows>>,
        order: Rc<RefCell<Vec<&'static str>>>,
    }

    impl GamePlugin for ProfileProbe {
        fn name(&self) -> &str {
            "profile_probe"
        }

        fn build(&mut self, sim: &mut Simulation) {
            sim.schedule_mut()
                .add_system(grimoire_ecs::system_fn("sigil.probe", |_| {}))
                .add_system(grimoire_ecs::system_fn("probe", |_| {}));
        }

        fn on_frame(&mut self, _stats: &FrameStats) {
            self.order.borrow_mut().push("on_frame");
        }

        fn on_profile(&mut self, profile: &grimoire_debug::FrameProfile) {
            self.order.borrow_mut().push("on_profile");
            let scopes = profile
                .scopes()
                .iter()
                .map(|total| {
                    (
                        profile
                            .scope_name(total.scope)
                            .unwrap_or_default()
                            .to_owned(),
                        total.total,
                        total.calls,
                        total.budget,
                        total.estimate,
                    )
                })
                .collect();
            self.rows.borrow_mut().push((profile.frame(), scopes));
        }
    }

    #[test]
    fn the_profiler_measures_every_loop_scope_with_the_platform_clock() {
        let probe = ProfileProbe::default();
        let rows = Rc::clone(&probe.rows);
        let order = Rc::clone(&probe.order);
        let factory: RendererFactory<ScriptedRenderer> =
            Box::new(|_| Ok(ScriptedRenderer::new(u64::MAX, || RenderError::SurfaceLost)));
        let mut game_loop = GameLoop::new(
            settings(),
            vec![Box::new(probe)],
            factory,
            Outcome::default(),
        );
        let mut ctx = SteppingContext::default();
        game_loop.init(&mut ctx).expect("init succeeds");
        for _ in 0..3 {
            ctx.clock.base.advance(Duration::from_nanos(16_666_667));
            game_loop.frame(&mut ctx);
        }
        assert_eq!(
            *order.borrow(),
            [
                "on_frame",
                "on_profile",
                "on_frame",
                "on_profile",
                "on_frame",
                "on_profile"
            ]
        );
        let rows = rows.borrow();
        let (frame, scopes) = &rows[1];
        assert_eq!(*frame, 1);
        let names: Vec<&str> = scopes.iter().map(|row| row.0.as_str()).collect();
        // Subsystem scopes appear during the tick, before the loop's own scopes.
        assert_eq!(
            names,
            ["sigil", "app", "sim", "extract", "render", "gpu", "frame"]
        );
        let scope = |name: &str| {
            scopes
                .iter()
                .find(|row| row.0 == name)
                .cloned()
                .expect("scope recorded")
        };
        for name in ["sigil", "app", "sim", "extract", "render", "frame"] {
            let (_, total, calls, _, estimate) = scope(name);
            assert!(total > Duration::ZERO, "{name} measured");
            assert_eq!(calls, 1, "{name} recorded once");
            assert!(!estimate, "{name} is a real measurement");
        }
        let (_, sim, ..) = scope("sim");
        let (_, sigil, ..) = scope("sigil");
        let (_, app, ..) = scope("app");
        assert!(sim > sigil + app, "sim contains its systems");
        let (_, extract, ..) = scope("extract");
        let (_, render, ..) = scope("render");
        let (_, frame_total, _, frame_budget, _) = scope("frame");
        assert!(frame_total > sim + extract + render);
        assert_eq!(frame_budget, Some(Duration::from_nanos(16_666_667)));
        // A renderer without timestamp queries: the render time stands in for the GPU, marked.
        let (_, gpu, gpu_calls, gpu_budget, gpu_estimate) = scope("gpu");
        assert_eq!(
            (gpu, gpu_calls, gpu_budget, gpu_estimate),
            (render, 1, Some(Duration::from_millis(8)), true)
        );
    }

    #[test]
    fn without_the_profiler_on_profile_is_never_called() {
        let probe = ProfileProbe::default();
        let order = Rc::clone(&probe.order);
        let factory: RendererFactory<ScriptedRenderer> =
            Box::new(|_| Ok(ScriptedRenderer::new(u64::MAX, || RenderError::SurfaceLost)));
        let mut settings = settings();
        settings.profiler = None;
        let mut game_loop =
            GameLoop::new(settings, vec![Box::new(probe)], factory, Outcome::default());
        run_headless(&mut game_loop, 5, Duration::from_millis(16)).expect("init succeeds");
        assert_eq!(*order.borrow(), ["on_frame"; 5]);
    }

    #[test]
    fn fps_is_averaged_over_one_second_windows() {
        let mut counter = FpsCounter::new(Duration::ZERO);
        let step = Duration::from_millis(10);
        let mut now = Duration::ZERO;
        let mut fps = 0.0;
        for _ in 0..99 {
            now += step;
            fps = counter.frame(now);
        }
        assert_eq!(fps, 0.0, "no window has completed yet");
        now += step;
        fps = counter.frame(now);
        assert!((fps - 100.0).abs() < 1e-9, "fps = {fps}");
    }
}
