//! Desktop runner (Windows, macOS, Linux) on top of `winit`.

use crate::app::AppHandler;
use crate::error::PlatformError;
use crate::window::WindowConfig;

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
    let _ = (config, app);
    todo!("P0 stream platform: winit 0.30 ApplicationHandler runner")
}
