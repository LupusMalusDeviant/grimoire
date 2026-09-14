//! Desktop runner (Windows, macOS, Linux) on top of `winit`.
//!
//! All `winit` types stay private to this module.

use std::sync::Arc;
use std::time::{Duration, Instant};

use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, WindowHandle,
};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseScrollDelta, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::PhysicalKey;
use winit::window::{Window, WindowId};

use crate::app::{AppHandler, PlatformContext};
use crate::clock::{Clock, SystemClock};
use crate::error::PlatformError;
use crate::event::{KeyCode, MouseButton, PlatformEvent, RawInputEvent};
use crate::window::{PhysicalSize, PlatformWindow, WindowConfig};

/// Logical pixels that count as one scrolled line when a device reports pixel deltas
/// (touchpads, precision wheels). Matches the line height common in desktop toolkits.
pub(crate) const LOGICAL_PIXELS_PER_LINE: f64 = 40.0;

/// Frame interval while nothing can be presented. Frames keep running at this rate so a missed
/// "visible again" notification cannot stall the application.
pub(crate) const THROTTLED_FRAME_INTERVAL: Duration = Duration::from_millis(100);

/// Consecutive frames reported as not presented before the loop is throttled. A single lost or
/// outdated surface after a resize must not cause a visible hitch.
pub(crate) const UNPRESENTED_FRAMES_BEFORE_THROTTLE: u32 = 3;

/// Creates the main window and runs `app` inside the native event loop until the app requests
/// exit or the window is closed.
///
/// Blocks the calling thread until the loop ends.
///
/// On macOS this function may never return: quitting through the application menu (Cmd+Q)
/// terminates the process inside AppKit. The loop still ends with [`AppHandler::shutdown`], but no
/// [`crate::PlatformEvent::CloseRequested`] is delivered, destructors of `app` do not run, and code
/// after the call is skipped. [`AppHandler::shutdown`] is therefore the only end-of-run hook that
/// runs on every orderly end of the loop; it does not run when the operating system ends the
/// process (Windows session end, `SIGTERM` or `SIGINT`, Ctrl+C in a console).
///
/// # Errors
/// [`PlatformError::EventLoop`] or [`PlatformError::WindowCreation`] if the platform fails,
/// [`PlatformError::AppInit`] if [`AppHandler::init`] fails. The native event loop can be created
/// only once per process, so a second call returns [`PlatformError::EventLoop`].
///
/// # Panics
/// Panics if not called on the main thread, on every desktop platform (e.g. from a `#[test]` or a
/// spawned thread).
pub fn run_desktop<A: AppHandler + 'static>(
    config: WindowConfig,
    app: A,
) -> Result<(), PlatformError> {
    let event_loop =
        EventLoop::new().map_err(|error| PlatformError::EventLoop(error.to_string()))?;
    let mut runner = DesktopRunner {
        app,
        config,
        ctx: DesktopContext {
            window: None,
            clock: SystemClock::new(),
            exit_requested: false,
            frame_not_presented: false,
        },
        window: None,
        lifecycle: Lifecycle::default(),
        error: None,
    };
    let result = event_loop.run_app(&mut runner);
    if let Some(error) = runner.error.take() {
        return Err(error);
    }
    result.map_err(|error| PlatformError::EventLoop(error.to_string()))
}

/// On macOS the raw handles are only available on the main thread, and `set_title` or
/// `request_redraw` from another thread are forwarded to it.
struct DesktopWindow {
    window: Arc<Window>,
}

impl HasWindowHandle for DesktopWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        self.window.window_handle()
    }
}

impl HasDisplayHandle for DesktopWindow {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        self.window.display_handle()
    }
}

impl PlatformWindow for DesktopWindow {
    fn inner_size(&self) -> PhysicalSize {
        let size = self.window.inner_size();
        PhysicalSize::new(size.width, size.height)
    }

    fn scale_factor(&self) -> f64 {
        self.window.scale_factor()
    }

    fn set_title(&self, title: &str) {
        self.window.set_title(title);
    }

    fn request_redraw(&self) {
        self.window.request_redraw();
    }
}

struct DesktopContext {
    window: Option<Arc<dyn PlatformWindow>>,
    clock: SystemClock,
    exit_requested: bool,
    frame_not_presented: bool,
}

impl PlatformContext for DesktopContext {
    fn window(&self) -> Option<Arc<dyn PlatformWindow>> {
        self.window.clone()
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
        self.frame_not_presented = true;
    }
}

/// When the next frame is requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FramePacing {
    /// Right away; the presentation paces the loop.
    Immediate,
    /// After the given delay, because nothing can be presented.
    After(Duration),
}

/// Lifecycle gating of the desktop runner, kept free of winit so it can be unit-tested.
#[derive(Debug, Default)]
struct Lifecycle {
    window_created: bool,
    initialized: bool,
    /// Set once the loop was told to exit; no app callback runs afterwards except `shutdown`.
    stopping: bool,
    shut_down: bool,
    occluded: bool,
    drawable_empty: bool,
    unpresented_frames: u32,
}

impl Lifecycle {
    /// Whether `resumed` should create the window: only the first time and never while stopping.
    fn should_create_window(&self) -> bool {
        !self.window_created && !self.stopping
    }

    fn window_created(&mut self) {
        self.window_created = true;
    }

    /// Records the outcome of `init`. Returns whether the event loop must exit.
    fn init_finished(&mut self, succeeded: bool, exit_requested: bool) -> bool {
        self.initialized = succeeded;
        self.callback_finished(!succeeded || exit_requested)
    }

    /// Whether `event` and `frame` may be delivered.
    fn accepts_callbacks(&self) -> bool {
        self.initialized && !self.stopping
    }

    /// Records the end of an app callback or a platform failure. Returns whether the event loop
    /// must exit.
    fn callback_finished(&mut self, exit: bool) -> bool {
        self.stopping |= exit;
        self.stopping
    }

    /// Records a change of the occlusion state. Returns whether the window became presentable
    /// again, so the next frame must be requested right away.
    fn set_occluded(&mut self, occluded: bool) -> bool {
        self.change_visibility(|lifecycle| lifecycle.occluded = occluded)
    }

    /// Records a new drawable size. Returns whether the window became presentable again.
    fn set_drawable_empty(&mut self, empty: bool) -> bool {
        self.change_visibility(|lifecycle| lifecycle.drawable_empty = empty)
    }

    fn change_visibility(&mut self, change: impl FnOnce(&mut Self)) -> bool {
        let was_hidden = self.is_hidden();
        change(self);
        let shown = was_hidden && !self.is_hidden();
        if shown {
            self.unpresented_frames = 0;
        }
        shown && self.accepts_callbacks()
    }

    fn is_hidden(&self) -> bool {
        self.occluded || self.drawable_empty
    }

    /// Records the end of a frame and decides when the next one is requested.
    fn frame_finished(&mut self, presented: bool) -> FramePacing {
        self.unpresented_frames = if presented {
            0
        } else {
            self.unpresented_frames.saturating_add(1)
        };
        if self.is_hidden() || self.unpresented_frames >= UNPRESENTED_FRAMES_BEFORE_THROTTLE {
            FramePacing::After(THROTTLED_FRAME_INTERVAL)
        } else {
            FramePacing::Immediate
        }
    }

    /// Returns `true` exactly once, and only if `init` succeeded.
    fn take_shutdown(&mut self) -> bool {
        let run = self.initialized && !self.shut_down;
        self.shut_down |= run;
        run
    }
}

struct DesktopRunner<A> {
    app: A,
    config: WindowConfig,
    ctx: DesktopContext,
    window: Option<Arc<Window>>,
    lifecycle: Lifecycle,
    error: Option<PlatformError>,
}

impl<A: AppHandler> DesktopRunner<A> {
    /// Returns whether the loop keeps running.
    fn finish_callback(&mut self, event_loop: &ActiveEventLoop, exit: bool) -> bool {
        let stopping = self
            .lifecycle
            .callback_finished(exit || self.ctx.exit_requested);
        if stopping {
            event_loop.exit();
        }
        !stopping
    }

    fn deliver(&mut self, event_loop: &ActiveEventLoop, event: &PlatformEvent) {
        self.app.event(&mut self.ctx, event);
        self.finish_callback(event_loop, false);
    }

    /// Requests the next frame now; `ControlFlow::Wait` cancels a pending throttle deadline.
    fn request_frame_now(event_loop: &ActiveEventLoop, window: &Window) {
        event_loop.set_control_flow(ControlFlow::Wait);
        window.request_redraw();
    }
}

impl<A: AppHandler> ApplicationHandler for DesktopRunner<A> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if !self.lifecycle.should_create_window() {
            return;
        }

        let attributes = Window::default_attributes()
            .with_title(self.config.title.clone())
            .with_inner_size(LogicalSize::new(
                f64::from(self.config.width),
                f64::from(self.config.height),
            ))
            .with_resizable(self.config.resizable);
        let attributes = crate::monitor::apply_placement(attributes, event_loop, &self.config);
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                self.error = Some(PlatformError::WindowCreation(error.to_string()));
                self.finish_callback(event_loop, true);
                return;
            }
        };
        self.lifecycle.window_created();
        self.window = Some(Arc::clone(&window));
        self.ctx.window = Some(Arc::new(DesktopWindow {
            window: Arc::clone(&window),
        }));

        let init_result = self.app.init(&mut self.ctx);
        let succeeded = init_result.is_ok();
        if let Err(error) = init_result {
            self.error = Some(PlatformError::AppInit(error.to_string()));
        }
        if self
            .lifecycle
            .init_finished(succeeded, self.ctx.exit_requested)
        {
            event_loop.exit();
        } else {
            window.request_redraw();
        }
    }

    fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: StartCause) {
        if let StartCause::ResumeTimeReached { .. } = cause
            && self.lifecycle.accepts_callbacks()
            && let Some(window) = &self.window
        {
            Self::request_frame_now(event_loop, window);
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(window) = self.window.clone() else {
            return;
        };
        if !self.lifecycle.accepts_callbacks() || window.id() != window_id {
            return;
        }

        match event {
            WindowEvent::RedrawRequested => {
                self.app.frame(&mut self.ctx);
                let presented = !std::mem::take(&mut self.ctx.frame_not_presented);
                if self.finish_callback(event_loop, false) {
                    match self.lifecycle.frame_finished(presented) {
                        FramePacing::Immediate => Self::request_frame_now(event_loop, &window),
                        FramePacing::After(delay) => event_loop
                            .set_control_flow(ControlFlow::WaitUntil(Instant::now() + delay)),
                    }
                }
            }
            WindowEvent::CloseRequested => {
                self.app
                    .event(&mut self.ctx, &PlatformEvent::CloseRequested);
                self.finish_callback(event_loop, true);
            }
            WindowEvent::Resized(size) => {
                let size = PhysicalSize::new(size.width, size.height);
                self.deliver(event_loop, &PlatformEvent::Resized(size));
                if self.lifecycle.set_drawable_empty(size.is_empty()) {
                    Self::request_frame_now(event_loop, &window);
                }
            }
            WindowEvent::Occluded(occluded) => {
                self.deliver(event_loop, &PlatformEvent::Occluded(occluded));
                if self.lifecycle.set_occluded(occluded) {
                    Self::request_frame_now(event_loop, &window);
                }
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.deliver(event_loop, &PlatformEvent::ScaleFactorChanged(scale_factor));
            }
            WindowEvent::Focused(focused) => {
                self.deliver(event_loop, &PlatformEvent::Focused(focused));
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let input = RawInputEvent::Key {
                    code: map_key_code(event.physical_key),
                    pressed: event.state == ElementState::Pressed,
                    repeat: event.repeat,
                };
                self.deliver(event_loop, &PlatformEvent::Input(input));
            }
            WindowEvent::CursorMoved { position, .. } => {
                let input = RawInputEvent::CursorMoved {
                    x: position.x,
                    y: position.y,
                };
                self.deliver(event_loop, &PlatformEvent::Input(input));
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let input = RawInputEvent::MouseButton {
                    button: map_mouse_button(button),
                    pressed: state == ElementState::Pressed,
                };
                self.deliver(event_loop, &PlatformEvent::Input(input));
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (delta_x, delta_y) = scroll_delta_to_lines(delta, window.scale_factor());
                let input = RawInputEvent::MouseWheel { delta_x, delta_y };
                self.deliver(event_loop, &PlatformEvent::Input(input));
            }
            _ => {}
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        if self.lifecycle.take_shutdown() {
            self.app.shutdown();
        }
    }
}

/// Converts a wheel delta to lines. Pixel deltas are first scaled to logical pixels so the
/// scroll distance does not depend on the display density.
pub(crate) fn scroll_delta_to_lines(delta: MouseScrollDelta, scale_factor: f64) -> (f32, f32) {
    match delta {
        MouseScrollDelta::LineDelta(x, y) => (x, y),
        MouseScrollDelta::PixelDelta(position) => {
            let scale = if scale_factor.is_finite() && scale_factor > 0.0 {
                scale_factor
            } else {
                1.0
            };
            let pixels_per_line = LOGICAL_PIXELS_PER_LINE * scale;
            (
                (position.x / pixels_per_line) as f32,
                (position.y / pixels_per_line) as f32,
            )
        }
    }
}

pub(crate) fn map_mouse_button(button: winit::event::MouseButton) -> MouseButton {
    use winit::event::MouseButton as W;
    match button {
        W::Left => MouseButton::Left,
        W::Right => MouseButton::Right,
        W::Middle => MouseButton::Middle,
        W::Back => MouseButton::Back,
        W::Forward => MouseButton::Forward,
        W::Other(index) => MouseButton::Other(index),
    }
}

/// Maps a layout-independent winit key to [`KeyCode`]; keys outside the enum become
/// [`KeyCode::Unidentified`].
pub(crate) fn map_key_code(key: PhysicalKey) -> KeyCode {
    use winit::keyboard::KeyCode as W;
    let PhysicalKey::Code(code) = key else {
        return KeyCode::Unidentified;
    };
    match code {
        W::KeyA => KeyCode::KeyA,
        W::KeyB => KeyCode::KeyB,
        W::KeyC => KeyCode::KeyC,
        W::KeyD => KeyCode::KeyD,
        W::KeyE => KeyCode::KeyE,
        W::KeyF => KeyCode::KeyF,
        W::KeyG => KeyCode::KeyG,
        W::KeyH => KeyCode::KeyH,
        W::KeyI => KeyCode::KeyI,
        W::KeyJ => KeyCode::KeyJ,
        W::KeyK => KeyCode::KeyK,
        W::KeyL => KeyCode::KeyL,
        W::KeyM => KeyCode::KeyM,
        W::KeyN => KeyCode::KeyN,
        W::KeyO => KeyCode::KeyO,
        W::KeyP => KeyCode::KeyP,
        W::KeyQ => KeyCode::KeyQ,
        W::KeyR => KeyCode::KeyR,
        W::KeyS => KeyCode::KeyS,
        W::KeyT => KeyCode::KeyT,
        W::KeyU => KeyCode::KeyU,
        W::KeyV => KeyCode::KeyV,
        W::KeyW => KeyCode::KeyW,
        W::KeyX => KeyCode::KeyX,
        W::KeyY => KeyCode::KeyY,
        W::KeyZ => KeyCode::KeyZ,
        W::Digit0 => KeyCode::Digit0,
        W::Digit1 => KeyCode::Digit1,
        W::Digit2 => KeyCode::Digit2,
        W::Digit3 => KeyCode::Digit3,
        W::Digit4 => KeyCode::Digit4,
        W::Digit5 => KeyCode::Digit5,
        W::Digit6 => KeyCode::Digit6,
        W::Digit7 => KeyCode::Digit7,
        W::Digit8 => KeyCode::Digit8,
        W::Digit9 => KeyCode::Digit9,
        W::ArrowUp => KeyCode::ArrowUp,
        W::ArrowDown => KeyCode::ArrowDown,
        W::ArrowLeft => KeyCode::ArrowLeft,
        W::ArrowRight => KeyCode::ArrowRight,
        W::Space => KeyCode::Space,
        W::Enter => KeyCode::Enter,
        W::Escape => KeyCode::Escape,
        W::Tab => KeyCode::Tab,
        W::Backspace => KeyCode::Backspace,
        W::ShiftLeft => KeyCode::ShiftLeft,
        W::ShiftRight => KeyCode::ShiftRight,
        W::ControlLeft => KeyCode::ControlLeft,
        W::ControlRight => KeyCode::ControlRight,
        W::AltLeft => KeyCode::AltLeft,
        W::AltRight => KeyCode::AltRight,
        W::F1 => KeyCode::F1,
        W::F2 => KeyCode::F2,
        W::F3 => KeyCode::F3,
        W::F4 => KeyCode::F4,
        W::F5 => KeyCode::F5,
        W::F6 => KeyCode::F6,
        W::F7 => KeyCode::F7,
        W::F8 => KeyCode::F8,
        W::F9 => KeyCode::F9,
        W::F10 => KeyCode::F10,
        W::F11 => KeyCode::F11,
        W::F12 => KeyCode::F12,
        _ => KeyCode::Unidentified,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::dpi::PhysicalPosition;
    use winit::keyboard::{KeyCode as W, NativeKeyCode};

    /// Every [`KeyCode`] variant, in declaration order.
    const ALL_KEY_CODES: [KeyCode; 64] = [
        KeyCode::KeyA,
        KeyCode::KeyB,
        KeyCode::KeyC,
        KeyCode::KeyD,
        KeyCode::KeyE,
        KeyCode::KeyF,
        KeyCode::KeyG,
        KeyCode::KeyH,
        KeyCode::KeyI,
        KeyCode::KeyJ,
        KeyCode::KeyK,
        KeyCode::KeyL,
        KeyCode::KeyM,
        KeyCode::KeyN,
        KeyCode::KeyO,
        KeyCode::KeyP,
        KeyCode::KeyQ,
        KeyCode::KeyR,
        KeyCode::KeyS,
        KeyCode::KeyT,
        KeyCode::KeyU,
        KeyCode::KeyV,
        KeyCode::KeyW,
        KeyCode::KeyX,
        KeyCode::KeyY,
        KeyCode::KeyZ,
        KeyCode::Digit0,
        KeyCode::Digit1,
        KeyCode::Digit2,
        KeyCode::Digit3,
        KeyCode::Digit4,
        KeyCode::Digit5,
        KeyCode::Digit6,
        KeyCode::Digit7,
        KeyCode::Digit8,
        KeyCode::Digit9,
        KeyCode::ArrowUp,
        KeyCode::ArrowDown,
        KeyCode::ArrowLeft,
        KeyCode::ArrowRight,
        KeyCode::Space,
        KeyCode::Enter,
        KeyCode::Escape,
        KeyCode::Tab,
        KeyCode::Backspace,
        KeyCode::ShiftLeft,
        KeyCode::ShiftRight,
        KeyCode::ControlLeft,
        KeyCode::ControlRight,
        KeyCode::AltLeft,
        KeyCode::AltRight,
        KeyCode::F1,
        KeyCode::F2,
        KeyCode::F3,
        KeyCode::F4,
        KeyCode::F5,
        KeyCode::F6,
        KeyCode::F7,
        KeyCode::F8,
        KeyCode::F9,
        KeyCode::F10,
        KeyCode::F11,
        KeyCode::F12,
        KeyCode::Unidentified,
    ];

    /// The winit key each engine key is expected to come from. The match has no wildcard, so
    /// adding a [`KeyCode`] variant stops this test from compiling until it is listed here and in
    /// [`ALL_KEY_CODES`].
    fn expected_winit_code(code: KeyCode) -> Option<W> {
        match code {
            KeyCode::KeyA => Some(W::KeyA),
            KeyCode::KeyB => Some(W::KeyB),
            KeyCode::KeyC => Some(W::KeyC),
            KeyCode::KeyD => Some(W::KeyD),
            KeyCode::KeyE => Some(W::KeyE),
            KeyCode::KeyF => Some(W::KeyF),
            KeyCode::KeyG => Some(W::KeyG),
            KeyCode::KeyH => Some(W::KeyH),
            KeyCode::KeyI => Some(W::KeyI),
            KeyCode::KeyJ => Some(W::KeyJ),
            KeyCode::KeyK => Some(W::KeyK),
            KeyCode::KeyL => Some(W::KeyL),
            KeyCode::KeyM => Some(W::KeyM),
            KeyCode::KeyN => Some(W::KeyN),
            KeyCode::KeyO => Some(W::KeyO),
            KeyCode::KeyP => Some(W::KeyP),
            KeyCode::KeyQ => Some(W::KeyQ),
            KeyCode::KeyR => Some(W::KeyR),
            KeyCode::KeyS => Some(W::KeyS),
            KeyCode::KeyT => Some(W::KeyT),
            KeyCode::KeyU => Some(W::KeyU),
            KeyCode::KeyV => Some(W::KeyV),
            KeyCode::KeyW => Some(W::KeyW),
            KeyCode::KeyX => Some(W::KeyX),
            KeyCode::KeyY => Some(W::KeyY),
            KeyCode::KeyZ => Some(W::KeyZ),
            KeyCode::Digit0 => Some(W::Digit0),
            KeyCode::Digit1 => Some(W::Digit1),
            KeyCode::Digit2 => Some(W::Digit2),
            KeyCode::Digit3 => Some(W::Digit3),
            KeyCode::Digit4 => Some(W::Digit4),
            KeyCode::Digit5 => Some(W::Digit5),
            KeyCode::Digit6 => Some(W::Digit6),
            KeyCode::Digit7 => Some(W::Digit7),
            KeyCode::Digit8 => Some(W::Digit8),
            KeyCode::Digit9 => Some(W::Digit9),
            KeyCode::ArrowUp => Some(W::ArrowUp),
            KeyCode::ArrowDown => Some(W::ArrowDown),
            KeyCode::ArrowLeft => Some(W::ArrowLeft),
            KeyCode::ArrowRight => Some(W::ArrowRight),
            KeyCode::Space => Some(W::Space),
            KeyCode::Enter => Some(W::Enter),
            KeyCode::Escape => Some(W::Escape),
            KeyCode::Tab => Some(W::Tab),
            KeyCode::Backspace => Some(W::Backspace),
            KeyCode::ShiftLeft => Some(W::ShiftLeft),
            KeyCode::ShiftRight => Some(W::ShiftRight),
            KeyCode::ControlLeft => Some(W::ControlLeft),
            KeyCode::ControlRight => Some(W::ControlRight),
            KeyCode::AltLeft => Some(W::AltLeft),
            KeyCode::AltRight => Some(W::AltRight),
            KeyCode::F1 => Some(W::F1),
            KeyCode::F2 => Some(W::F2),
            KeyCode::F3 => Some(W::F3),
            KeyCode::F4 => Some(W::F4),
            KeyCode::F5 => Some(W::F5),
            KeyCode::F6 => Some(W::F6),
            KeyCode::F7 => Some(W::F7),
            KeyCode::F8 => Some(W::F8),
            KeyCode::F9 => Some(W::F9),
            KeyCode::F10 => Some(W::F10),
            KeyCode::F11 => Some(W::F11),
            KeyCode::F12 => Some(W::F12),
            KeyCode::Unidentified => None,
        }
    }

    #[test]
    fn all_key_codes_lists_every_variant_once() {
        // Declaration-order discriminants are dense, so a variant missing from the list leaves a
        // gap or a length mismatch.
        let mut discriminants: Vec<usize> =
            ALL_KEY_CODES.iter().map(|code| *code as usize).collect();
        discriminants.sort_unstable();
        let dense: Vec<usize> = (0..ALL_KEY_CODES.len()).collect();
        assert_eq!(discriminants, dense);
    }

    #[test]
    fn maps_every_named_key() {
        for code in ALL_KEY_CODES {
            match expected_winit_code(code) {
                Some(winit_code) => assert_eq!(
                    map_key_code(PhysicalKey::Code(winit_code)),
                    code,
                    "{winit_code:?}"
                ),
                None => assert_eq!(code, KeyCode::Unidentified),
            }
        }
    }

    #[test]
    fn lifecycle_normal_run_shuts_down_once() {
        let mut lifecycle = Lifecycle::default();
        assert!(lifecycle.should_create_window());
        lifecycle.window_created();
        assert!(
            !lifecycle.should_create_window(),
            "later resumes are ignored"
        );
        assert!(!lifecycle.accepts_callbacks(), "no events before init");

        assert!(!lifecycle.init_finished(true, false));
        assert!(lifecycle.accepts_callbacks());
        assert!(!lifecycle.callback_finished(false));
        assert!(lifecycle.accepts_callbacks());

        assert!(lifecycle.callback_finished(true));
        assert!(
            !lifecycle.accepts_callbacks(),
            "nothing is delivered once stopping"
        );
        assert!(lifecycle.callback_finished(false), "stopping is sticky");

        assert!(lifecycle.take_shutdown());
        assert!(!lifecycle.take_shutdown());
    }

    #[test]
    fn lifecycle_init_error_exits_without_shutdown() {
        let mut lifecycle = Lifecycle::default();
        lifecycle.window_created();
        assert!(lifecycle.init_finished(false, false));
        assert!(!lifecycle.accepts_callbacks());
        assert!(!lifecycle.take_shutdown());
    }

    #[test]
    fn lifecycle_exit_requested_in_init_skips_frames() {
        let mut lifecycle = Lifecycle::default();
        lifecycle.window_created();
        assert!(lifecycle.init_finished(true, true));
        assert!(!lifecycle.accepts_callbacks());
        assert!(lifecycle.take_shutdown());
        assert!(!lifecycle.take_shutdown());
    }

    #[test]
    fn lifecycle_window_creation_failure_never_inits() {
        let mut lifecycle = Lifecycle::default();
        assert!(lifecycle.callback_finished(true));
        assert!(!lifecycle.should_create_window());
        assert!(!lifecycle.accepts_callbacks());
        assert!(!lifecycle.take_shutdown());
    }

    fn running_lifecycle() -> Lifecycle {
        let mut lifecycle = Lifecycle::default();
        lifecycle.window_created();
        assert!(!lifecycle.init_finished(true, false));
        lifecycle
    }

    #[test]
    fn presented_frames_request_the_next_frame_immediately() {
        let mut lifecycle = running_lifecycle();
        for _ in 0..100 {
            assert_eq!(lifecycle.frame_finished(true), FramePacing::Immediate);
        }
    }

    #[test]
    fn minimised_window_throttles_until_it_has_a_size_again() {
        let mut lifecycle = running_lifecycle();
        assert!(!lifecycle.set_drawable_empty(true));
        for _ in 0..10 {
            assert_eq!(
                lifecycle.frame_finished(true),
                FramePacing::After(THROTTLED_FRAME_INTERVAL)
            );
        }
        assert!(!lifecycle.set_drawable_empty(true), "still empty");
        assert!(lifecycle.set_drawable_empty(false), "restored: frame now");
        assert!(!lifecycle.set_drawable_empty(false), "no second wake-up");
        assert_eq!(lifecycle.frame_finished(true), FramePacing::Immediate);
    }

    #[test]
    fn occluded_window_throttles_until_visible_again() {
        let mut lifecycle = running_lifecycle();
        assert!(!lifecycle.set_occluded(true));
        assert_eq!(
            lifecycle.frame_finished(false),
            FramePacing::After(THROTTLED_FRAME_INTERVAL)
        );
        assert!(lifecycle.set_occluded(false));
        assert_eq!(lifecycle.frame_finished(true), FramePacing::Immediate);
    }

    #[test]
    fn window_stays_throttled_while_any_hiding_reason_remains() {
        let mut lifecycle = running_lifecycle();
        assert!(!lifecycle.set_occluded(true));
        assert!(!lifecycle.set_drawable_empty(true));
        assert!(!lifecycle.set_occluded(false), "still minimised");
        assert_eq!(
            lifecycle.frame_finished(true),
            FramePacing::After(THROTTLED_FRAME_INTERVAL)
        );
        assert!(lifecycle.set_drawable_empty(false));
        assert_eq!(lifecycle.frame_finished(true), FramePacing::Immediate);
    }

    #[test]
    fn repeated_unpresented_frames_throttle_until_a_frame_presents() {
        let mut lifecycle = running_lifecycle();
        for _ in 1..UNPRESENTED_FRAMES_BEFORE_THROTTLE {
            assert_eq!(lifecycle.frame_finished(false), FramePacing::Immediate);
        }
        for _ in 0..10 {
            assert_eq!(
                lifecycle.frame_finished(false),
                FramePacing::After(THROTTLED_FRAME_INTERVAL)
            );
        }
        assert_eq!(lifecycle.frame_finished(true), FramePacing::Immediate);
        assert_eq!(
            lifecycle.frame_finished(false),
            FramePacing::Immediate,
            "a presented frame resets the streak"
        );
    }

    #[test]
    fn becoming_visible_resets_the_unpresented_streak() {
        let mut lifecycle = running_lifecycle();
        assert!(!lifecycle.set_occluded(true));
        for _ in 0..UNPRESENTED_FRAMES_BEFORE_THROTTLE {
            lifecycle.frame_finished(false);
        }
        assert!(lifecycle.set_occluded(false));
        assert_eq!(lifecycle.frame_finished(false), FramePacing::Immediate);
    }

    #[test]
    fn becoming_visible_requests_no_frame_before_init_or_while_stopping() {
        let mut lifecycle = Lifecycle::default();
        lifecycle.window_created();
        assert!(!lifecycle.set_drawable_empty(true));
        assert!(
            !lifecycle.set_drawable_empty(false),
            "no frames before init"
        );

        let mut lifecycle = running_lifecycle();
        assert!(!lifecycle.set_occluded(true));
        assert!(lifecycle.callback_finished(true));
        assert!(!lifecycle.set_occluded(false), "no frames once stopping");
    }

    #[test]
    fn unmapped_and_unidentified_keys_fall_back() {
        for code in [W::F13, W::Numpad5, W::CapsLock, W::Semicolon, W::SuperLeft] {
            assert_eq!(map_key_code(PhysicalKey::Code(code)), KeyCode::Unidentified);
        }
        for native in [
            NativeKeyCode::Unidentified,
            NativeKeyCode::Windows(0x1234),
            NativeKeyCode::Xkb(42),
        ] {
            assert_eq!(
                map_key_code(PhysicalKey::Unidentified(native)),
                KeyCode::Unidentified
            );
        }
    }

    #[test]
    fn maps_mouse_buttons() {
        use winit::event::MouseButton as B;
        assert_eq!(map_mouse_button(B::Left), MouseButton::Left);
        assert_eq!(map_mouse_button(B::Right), MouseButton::Right);
        assert_eq!(map_mouse_button(B::Middle), MouseButton::Middle);
        assert_eq!(map_mouse_button(B::Back), MouseButton::Back);
        assert_eq!(map_mouse_button(B::Forward), MouseButton::Forward);
        assert_eq!(map_mouse_button(B::Other(7)), MouseButton::Other(7));
    }

    #[test]
    fn line_deltas_pass_through() {
        assert_eq!(
            scroll_delta_to_lines(MouseScrollDelta::LineDelta(-1.0, 3.0), 2.0),
            (-1.0, 3.0)
        );
    }

    #[test]
    fn pixel_deltas_become_lines_in_logical_pixels() {
        let delta = MouseScrollDelta::PixelDelta(PhysicalPosition::new(80.0, -40.0));
        assert_eq!(scroll_delta_to_lines(delta, 1.0), (2.0, -1.0));
        assert_eq!(scroll_delta_to_lines(delta, 2.0), (1.0, -0.5));
        // Nonsensical scale factors fall back to 1.0 instead of producing inf/NaN.
        assert_eq!(scroll_delta_to_lines(delta, 0.0), (2.0, -1.0));
        assert_eq!(scroll_delta_to_lines(delta, f64::NAN), (2.0, -1.0));
    }
}
