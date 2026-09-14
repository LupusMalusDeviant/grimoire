//! The main loop shared by the desktop run and the headless frame loop.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use grimoire_platform::{
    AppHandler, AppResult, KeyCode, PlatformContext, PlatformEvent, RawInputEvent,
};
use grimoire_render::{RenderError, RenderFrame, RenderStats, Renderer};
use grimoire_sim::{FixedTimestep, Simulation, TickInput};

use crate::error::GrimoireError;
use crate::input::{InputMap, InputState};
use crate::plugin::{FrameStats, GamePlugin};

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
    pub record_hashes: bool,
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
pub(crate) struct GameLoop<R: Renderer> {
    settings: LoopSettings,
    plugins: Vec<Box<dyn GamePlugin>>,
    factory: Option<RendererFactory<R>>,
    running: Option<Running<R>>,
    input: InputState,
    render_frame: RenderFrame,
    timestep: FixedTimestep,
    last_time: Duration,
    fps: FpsCounter,
    frames: u64,
    hashes: Vec<(u64, u64)>,
    outcome: Outcome,
}

impl<R: Renderer> GameLoop<R> {
    pub(crate) fn new(
        settings: LoopSettings,
        plugins: Vec<Box<dyn GamePlugin>>,
        factory: RendererFactory<R>,
        outcome: Outcome,
    ) -> Self {
        let timestep = FixedTimestep::new(settings.tick_rate_hz)
            .with_max_ticks_per_frame(settings.max_ticks_per_frame);
        Self {
            settings,
            plugins,
            factory: Some(factory),
            running: None,
            input: InputState::new(),
            render_frame: RenderFrame::new(),
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

impl<R: Renderer> AppHandler for GameLoop<R> {
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
        for plugin in &mut self.plugins {
            log::debug!("building plugin {}", plugin.name());
            plugin.build(&mut sim);
        }
        if let Some(window) = ctx.window() {
            for plugin in &mut self.plugins {
                plugin.window_created(&window);
            }
        }

        self.last_time = ctx.clock().elapsed();
        self.fps = FpsCounter::new(self.last_time);
        self.running = Some(Running { sim, renderer });
        if self.settings.max_frames == Some(0) {
            ctx.request_exit();
        }
        Ok(())
    }

    fn event(&mut self, ctx: &mut dyn PlatformContext, event: &PlatformEvent) {
        match event {
            PlatformEvent::Resized(size) => {
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
                    && self.settings.exit_key == Some(code)
                {
                    ctx.request_exit();
                }
                self.input.apply(raw);
            }
            PlatformEvent::Focused(true)
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
        for _ in 0..plan.ticks {
            running.sim.step(tick_input);
            let tick = running.sim.tick();
            if self.settings.record_hashes
                && self.settings.hash_every != 0
                && tick.is_multiple_of(self.settings.hash_every)
            {
                self.hashes.push((tick, running.sim.state_hash()));
            }
        }

        self.render_frame.clear();
        for plugin in &mut self.plugins {
            plugin.extract(running.sim.world(), plan.alpha, &mut self.render_frame);
        }
        let render = match running.renderer.render(&self.render_frame) {
            Ok(stats) => stats,
            Err(RenderError::SurfaceLost) => RenderStats::default(),
            Err(error) => {
                log::error!("rendering failed, ending the run: {error}");
                self.fail(error.into());
                ctx.request_exit();
                return;
            }
        };

        let stats = FrameStats {
            frame: self.frames,
            sim_tick: running.sim.tick(),
            ticks_this_frame: plan.ticks,
            alpha: plan.alpha,
            frame_time,
            fps: self.fps.frame(now),
            dropped_time: self.timestep.dropped_time(),
            render,
        };
        for plugin in &mut self.plugins {
            plugin.on_frame(&stats);
        }
        self.frames += 1;
        if self
            .settings
            .max_frames
            .is_some_and(|max| self.frames >= max)
        {
            ctx.request_exit();
        }
    }
}

/// Feeds scripted platform events to a loop before each frame, for headless runs.
pub(crate) struct ScriptedEvents<'a, R: Renderer> {
    pub inner: &'a mut GameLoop<R>,
    pub script: &'a mut dyn FnMut(u64, &mut Vec<PlatformEvent>),
    pub events: Vec<PlatformEvent>,
    pub frame: u64,
}

impl<R: Renderer> AppHandler for ScriptedEvents<'_, R> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use grimoire_platform::run_headless;

    /// Fails with the scripted error on the given render call (0-based).
    struct ScriptedRenderer {
        calls: u64,
        fail_on: u64,
        error: fn() -> RenderError,
    }

    impl Renderer for ScriptedRenderer {
        fn resize(&mut self, _width: u32, _height: u32) {}

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
            record_hashes: true,
        }
    }

    fn run_scripted(
        fail_on: u64,
        error: fn() -> RenderError,
    ) -> (Outcome, Vec<FrameStats>, Option<LoopReport>) {
        let outcome = Outcome::default();
        let frames = Frames::default();
        let seen = Rc::clone(&frames.0);
        let factory: RendererFactory<ScriptedRenderer> = Box::new(move |_| {
            Ok(ScriptedRenderer {
                calls: 0,
                fail_on,
                error,
            })
        });
        let mut game_loop = GameLoop::new(
            settings(),
            vec![Box::new(frames)],
            factory,
            Rc::clone(&outcome),
        );
        run_headless(&mut game_loop, 10, Duration::from_millis(16)).expect("init succeeds");
        let report = game_loop.report();
        let seen = seen.borrow().clone();
        (outcome, seen, report)
    }

    #[test]
    fn surface_lost_skips_the_frame_and_keeps_running() {
        let (outcome, frames, report) = run_scripted(2, || RenderError::SurfaceLost);
        assert!(outcome.borrow().is_none());
        assert_eq!(frames.len(), 10);
        assert_eq!(frames[2].render, RenderStats::default());
        assert_eq!(frames[3].render.draw_calls, 1);
        assert_eq!(report.map(|report| report.frames), Some(10));
    }

    #[test]
    fn other_render_errors_end_the_run_with_the_error() {
        let (outcome, frames, report) = run_scripted(3, || RenderError::OutOfMemory);
        assert!(matches!(
            *outcome.borrow(),
            Some(GrimoireError::Render(RenderError::OutOfMemory))
        ));
        assert_eq!(frames.len(), 3, "on_frame is skipped for the failed frame");
        assert_eq!(report.map(|report| report.frames), Some(3));
    }

    #[test]
    fn renderer_creation_failure_is_kept_and_skips_build() {
        let outcome = Outcome::default();
        let factory: RendererFactory<ScriptedRenderer> =
            Box::new(|_| Err(RenderError::NoAdapter.into()));
        let mut game_loop = GameLoop::new(settings(), Vec::new(), factory, Rc::clone(&outcome));
        let error = run_headless(&mut game_loop, 5, Duration::from_millis(16)).unwrap_err();
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
