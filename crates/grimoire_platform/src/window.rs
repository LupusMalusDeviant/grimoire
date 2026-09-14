//! Native window contract consumed by the GPU layer.

use raw_window_handle::{HasDisplayHandle, HasWindowHandle};

/// Size of a drawable area in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PhysicalSize {
    /// Width in physical pixels.
    pub width: u32,
    /// Height in physical pixels.
    pub height: u32,
}

impl PhysicalSize {
    /// Creates a size from width and height.
    #[must_use]
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    /// Returns `true` if either dimension is zero (e.g. a minimised window).
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.width == 0 || self.height == 0
    }
}

/// Parameters for creating the main window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowConfig {
    /// Window title.
    pub title: String,
    /// Initial inner width in logical pixels.
    pub width: u32,
    /// Initial inner height in logical pixels.
    pub height: u32,
    /// Whether the user may resize the window.
    pub resizable: bool,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            title: String::from("Grimoire"),
            width: 1280,
            height: 720,
            resizable: true,
        }
    }
}

/// A native window a GPU surface can be created for.
///
/// Contract:
/// - Implementations are shared as `Arc<dyn PlatformWindow>` and must be thread-safe.
/// - The GPU layer relies only on the raw handles and [`PlatformWindow::inner_size`];
///   everything else is presentation glue.
/// - The handles stay valid for as long as any `Arc` to the window is alive.
///
/// Platform notes for the desktop implementation:
/// - **macOS:** the raw handles are only available on the main thread (other threads get
///   [`raw_window_handle::HandleError::Unavailable`]), so GPU surfaces must be created inside
///   the [`crate::AppHandler`] callbacks. Calls from other threads are forwarded to the main
///   thread and can block while it is busy.
/// - **Wayland:** redraws are not throttled to the compositor's frame callbacks; frame pacing
///   comes from presenting with vsync (FIFO).
pub trait PlatformWindow: HasWindowHandle + HasDisplayHandle + Send + Sync {
    /// Current drawable size in physical pixels. May be empty while minimised.
    fn inner_size(&self) -> PhysicalSize;

    /// Ratio of physical to logical pixels.
    fn scale_factor(&self) -> f64;

    /// Replaces the window title.
    fn set_title(&self, title: &str);

    /// Asks the platform to schedule another frame.
    fn request_redraw(&self);
}
