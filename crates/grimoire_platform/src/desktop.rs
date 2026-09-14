//! Desktop runner (Windows, macOS, Linux) on top of `winit`.
//!
//! All `winit` types stay private to this module.

use std::sync::Arc;

use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, WindowHandle,
};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
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

/// Creates the main window and runs `app` inside the native event loop until the app requests
/// exit or the window is closed.
///
/// Blocks the calling thread, which must be the main thread (required on macOS).
///
/// # Errors
/// [`PlatformError::EventLoop`] or [`PlatformError::WindowCreation`] if the platform fails,
/// [`PlatformError::AppInit`] if [`AppHandler::init`] fails.
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
        },
        window: None,
        initialized: false,
        stopping: false,
        shut_down: false,
        error: None,
    };
    let result = event_loop.run_app(&mut runner);
    if let Some(error) = runner.error.take() {
        return Err(error);
    }
    result.map_err(|error| PlatformError::EventLoop(error.to_string()))
}

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
}

struct DesktopRunner<A> {
    app: A,
    config: WindowConfig,
    ctx: DesktopContext,
    window: Option<Arc<Window>>,
    initialized: bool,
    /// Set once the loop was told to exit; no app callback runs afterwards except `shutdown`.
    stopping: bool,
    shut_down: bool,
    error: Option<PlatformError>,
}

impl<A: AppHandler> DesktopRunner<A> {
    fn stop(&mut self, event_loop: &ActiveEventLoop) {
        self.stopping = true;
        event_loop.exit();
    }

    fn stop_if_requested(&mut self, event_loop: &ActiveEventLoop) {
        if self.ctx.exit_requested {
            self.stop(event_loop);
        }
    }

    fn deliver(&mut self, event_loop: &ActiveEventLoop, event: &PlatformEvent) {
        self.app.event(&mut self.ctx, event);
        self.stop_if_requested(event_loop);
    }
}

impl<A: AppHandler> ApplicationHandler for DesktopRunner<A> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() || self.stopping {
            return;
        }

        let attributes = Window::default_attributes()
            .with_title(self.config.title.clone())
            .with_inner_size(LogicalSize::new(
                f64::from(self.config.width),
                f64::from(self.config.height),
            ))
            .with_resizable(self.config.resizable);
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                self.error = Some(PlatformError::WindowCreation(error.to_string()));
                self.stop(event_loop);
                return;
            }
        };
        self.window = Some(Arc::clone(&window));
        self.ctx.window = Some(Arc::new(DesktopWindow {
            window: Arc::clone(&window),
        }));

        if let Err(error) = self.app.init(&mut self.ctx) {
            self.error = Some(PlatformError::AppInit(error.to_string()));
            self.stop(event_loop);
            return;
        }
        self.initialized = true;
        self.stop_if_requested(event_loop);
        if !self.stopping {
            window.request_redraw();
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
        if !self.initialized || self.stopping || window.id() != window_id {
            return;
        }

        match event {
            WindowEvent::RedrawRequested => {
                self.app.frame(&mut self.ctx);
                self.stop_if_requested(event_loop);
                if !self.stopping {
                    window.request_redraw();
                }
            }
            WindowEvent::CloseRequested => {
                self.app
                    .event(&mut self.ctx, &PlatformEvent::CloseRequested);
                self.stop(event_loop);
            }
            WindowEvent::Resized(size) => {
                let event = PlatformEvent::Resized(PhysicalSize::new(size.width, size.height));
                self.deliver(event_loop, &event);
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
        if self.initialized && !self.shut_down {
            self.shut_down = true;
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

    #[test]
    fn maps_every_named_key() {
        let pairs = [
            (W::KeyA, KeyCode::KeyA),
            (W::KeyB, KeyCode::KeyB),
            (W::KeyC, KeyCode::KeyC),
            (W::KeyD, KeyCode::KeyD),
            (W::KeyE, KeyCode::KeyE),
            (W::KeyF, KeyCode::KeyF),
            (W::KeyG, KeyCode::KeyG),
            (W::KeyH, KeyCode::KeyH),
            (W::KeyI, KeyCode::KeyI),
            (W::KeyJ, KeyCode::KeyJ),
            (W::KeyK, KeyCode::KeyK),
            (W::KeyL, KeyCode::KeyL),
            (W::KeyM, KeyCode::KeyM),
            (W::KeyN, KeyCode::KeyN),
            (W::KeyO, KeyCode::KeyO),
            (W::KeyP, KeyCode::KeyP),
            (W::KeyQ, KeyCode::KeyQ),
            (W::KeyR, KeyCode::KeyR),
            (W::KeyS, KeyCode::KeyS),
            (W::KeyT, KeyCode::KeyT),
            (W::KeyU, KeyCode::KeyU),
            (W::KeyV, KeyCode::KeyV),
            (W::KeyW, KeyCode::KeyW),
            (W::KeyX, KeyCode::KeyX),
            (W::KeyY, KeyCode::KeyY),
            (W::KeyZ, KeyCode::KeyZ),
            (W::Digit0, KeyCode::Digit0),
            (W::Digit1, KeyCode::Digit1),
            (W::Digit2, KeyCode::Digit2),
            (W::Digit3, KeyCode::Digit3),
            (W::Digit4, KeyCode::Digit4),
            (W::Digit5, KeyCode::Digit5),
            (W::Digit6, KeyCode::Digit6),
            (W::Digit7, KeyCode::Digit7),
            (W::Digit8, KeyCode::Digit8),
            (W::Digit9, KeyCode::Digit9),
            (W::ArrowUp, KeyCode::ArrowUp),
            (W::ArrowDown, KeyCode::ArrowDown),
            (W::ArrowLeft, KeyCode::ArrowLeft),
            (W::ArrowRight, KeyCode::ArrowRight),
            (W::Space, KeyCode::Space),
            (W::Enter, KeyCode::Enter),
            (W::Escape, KeyCode::Escape),
            (W::Tab, KeyCode::Tab),
            (W::Backspace, KeyCode::Backspace),
            (W::ShiftLeft, KeyCode::ShiftLeft),
            (W::ShiftRight, KeyCode::ShiftRight),
            (W::ControlLeft, KeyCode::ControlLeft),
            (W::ControlRight, KeyCode::ControlRight),
            (W::AltLeft, KeyCode::AltLeft),
            (W::AltRight, KeyCode::AltRight),
            (W::F1, KeyCode::F1),
            (W::F2, KeyCode::F2),
            (W::F3, KeyCode::F3),
            (W::F4, KeyCode::F4),
            (W::F5, KeyCode::F5),
            (W::F6, KeyCode::F6),
            (W::F7, KeyCode::F7),
            (W::F8, KeyCode::F8),
            (W::F9, KeyCode::F9),
            (W::F10, KeyCode::F10),
            (W::F11, KeyCode::F11),
            (W::F12, KeyCode::F12),
        ];
        for (winit_code, expected) in pairs {
            assert_eq!(
                map_key_code(PhysicalKey::Code(winit_code)),
                expected,
                "{winit_code:?}"
            );
        }
        // Every non-fallback variant of KeyCode is covered exactly once.
        let mut mapped: Vec<KeyCode> = pairs.iter().map(|(_, code)| *code).collect();
        mapped.sort();
        mapped.dedup();
        assert_eq!(mapped.len(), pairs.len());
        assert!(!mapped.contains(&KeyCode::Unidentified));
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
