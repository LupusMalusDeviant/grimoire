//! Headless runner without any window — for tests, CI and the simulation harness.

use std::time::Duration;

use crate::app::AppHandler;
use crate::error::PlatformError;

/// Drives `app` for exactly `frames` frames without a window.
///
/// Before every frame a [`crate::ManualClock`] advances by `frame_delta`, so runs are fully
/// reproducible. `ctx.window()` returns `None`. The loop ends early if the app requests exit.
///
/// # Errors
/// [`PlatformError::AppInit`] if [`AppHandler::init`] fails.
pub fn run_headless<A: AppHandler>(
    app: &mut A,
    frames: u64,
    frame_delta: Duration,
) -> Result<(), PlatformError> {
    let _ = (app, frames, frame_delta);
    todo!("P0 stream platform")
}
