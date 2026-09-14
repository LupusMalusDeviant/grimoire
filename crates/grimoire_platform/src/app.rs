//! Application lifecycle driven by a platform runner.

use std::sync::Arc;

use crate::clock::Clock;
use crate::event::PlatformEvent;
use crate::window::PlatformWindow;

/// Result of fallible lifecycle hooks.
pub type AppResult = Result<(), Box<dyn std::error::Error + Send + Sync + 'static>>;

/// Services a runner offers to the application during lifecycle calls.
pub trait PlatformContext {
    /// The main window, or `None` when running headless.
    fn window(&self) -> Option<Arc<dyn PlatformWindow>>;

    /// Monotonic clock of this run (real time on desktop, manual time when headless).
    fn clock(&self) -> &dyn Clock;

    /// Asks the runner to end the loop after the current call returns.
    fn request_exit(&mut self);

    /// Whether an exit has been requested.
    fn exit_requested(&self) -> bool;

    /// Reports that the current frame presented nothing, e.g. because the surface was
    /// unavailable. After several such frames in a row the desktop runner slows the loop down
    /// instead of spinning; the headless runner ignores it.
    fn frame_not_presented(&mut self) {}
}

/// Application driven by [`crate::run_desktop`] or [`crate::run_headless`].
///
/// See the crate documentation for the exact call order.
pub trait AppHandler {
    /// Called exactly once before any event or frame. Returning an error aborts the run and the
    /// runner reports it as [`crate::PlatformError::AppInit`]; `shutdown` is not called then.
    fn init(&mut self, ctx: &mut dyn PlatformContext) -> AppResult;

    /// Called for every platform event, before the next frame.
    fn event(&mut self, ctx: &mut dyn PlatformContext, event: &PlatformEvent);

    /// Called once per frame.
    fn frame(&mut self, ctx: &mut dyn PlatformContext);

    /// Called exactly once when the loop ends after a successful `init`.
    ///
    /// The only end-of-run hook guaranteed on every platform: on macOS, quitting through the
    /// application menu (Cmd+Q) ends the process right after this call, without returning from
    /// [`crate::run_desktop`] and without running destructors. Failures here must be logged.
    fn shutdown(&mut self) {}
}
