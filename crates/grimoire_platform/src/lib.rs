//! # grimoire_platform
//!
//! Lowest engine layer: native windows, the OS event loop, raw input events, monotonic clocks
//! and file system access — each behind a trait, with a desktop implementation (Windows, macOS,
//! Linux via `winit`) and a headless implementation for tests and the simulation harness.
//!
//! Layer rule: depends on no other `grimoire_*` crate and never exposes `winit` types.
//!
//! ## Lifecycle contract
//!
//! [`run_desktop`] and [`run_headless`] drive an [`AppHandler`]:
//! 1. [`AppHandler::init`] exactly once — on desktop after the window exists.
//! 2. Any number of [`AppHandler::event`] calls, always delivered before the next frame.
//! 3. [`AppHandler::frame`] continuously, once per presented frame.
//! 4. [`AppHandler::shutdown`] exactly once when the loop ends (exit requested or window closed),
//!    but only if `init` succeeded. A failed `init` ends the run with [`PlatformError::AppInit`].
//!
//! `shutdown` is the only end-of-run hook that is guaranteed: on macOS, Cmd+Q ends the process
//! right after it, so [`run_desktop`] does not return there and no `CloseRequested` event arrives.
//!
//! On desktop, [`PlatformContext::request_exit`] is honoured after every callback; no further
//! `event` or `frame` call follows it. Wheel deltas reported in pixels are converted to lines at
//! 40 logical pixels per line.
//!
//! The desktop loop requests the next redraw right after every `frame` and leaves pacing to the
//! presentation (vsync). While nothing can be presented it slows down to one frame every 100 ms
//! instead of spinning: while the window is occluded ([`PlatformEvent::Occluded`]) or has an
//! empty drawable size (minimised), and after three frames in a row reported through
//! [`PlatformContext::frame_not_presented`]. It returns to full speed as soon as the window is
//! visible again or a frame presents. On Wayland winit aligns redraws with frame callbacks only
//! through `pre_present_notify`, which [`PlatformWindow`] does not expose, so the renderer must
//! present with vsync (FIFO) to avoid a spinning loop.
//!
//! ## Window placement during development
//!
//! [`WindowConfig::monitor`] and [`WindowConfig::focus_on_open`] choose where the window opens
//! and whether it takes focus. The environment variables [`ENV_WINDOW_MONITOR`]
//! (`GRIMOIRE_WINDOW_MONITOR=secondary`) and [`ENV_WINDOW_FOCUS`] (`GRIMOIRE_WINDOW_FOCUS=0`)
//! override both for any program built on this crate, so examples and smoke tests can open on a
//! second monitor without stealing focus from the application in front.

pub mod app;
pub mod clock;
pub mod desktop;
pub mod error;
pub mod event;
pub mod fs;
pub mod headless;
mod monitor;
pub mod window;

pub use app::{AppHandler, AppResult, PlatformContext};
pub use clock::{Clock, ManualClock, SystemClock};
pub use desktop::run_desktop;
pub use error::PlatformError;
pub use event::{KeyCode, MouseButton, PlatformEvent, RawInputEvent};
pub use fs::{FileSystem, MemoryFileSystem, StdFileSystem};
pub use headless::run_headless;
pub use raw_window_handle;
pub use window::{
    ENV_WINDOW_FOCUS, ENV_WINDOW_MONITOR, MonitorChoice, PhysicalSize, PlatformWindow, WindowConfig,
};
