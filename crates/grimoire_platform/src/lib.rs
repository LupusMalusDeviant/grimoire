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
//! On desktop, [`PlatformContext::request_exit`] is honoured after every callback; no further
//! `event` or `frame` call follows it. Wheel deltas reported in pixels are converted to lines at
//! 40 logical pixels per line.
//!
//! The desktop loop requests the next redraw right after every `frame` and does not pace itself.
//! On Wayland winit aligns redraws with frame callbacks only through `pre_present_notify`, which
//! [`PlatformWindow`] does not expose, so the renderer must present with vsync (FIFO) to avoid a
//! spinning loop.

pub mod app;
pub mod clock;
pub mod desktop;
pub mod error;
pub mod event;
pub mod fs;
pub mod headless;
pub mod window;

pub use app::{AppHandler, AppResult, PlatformContext};
pub use clock::{Clock, ManualClock, SystemClock};
pub use desktop::run_desktop;
pub use error::PlatformError;
pub use event::{KeyCode, MouseButton, PlatformEvent, RawInputEvent};
pub use fs::{FileSystem, MemoryFileSystem, StdFileSystem};
pub use headless::run_headless;
pub use raw_window_handle;
pub use window::{PhysicalSize, PlatformWindow, WindowConfig};
