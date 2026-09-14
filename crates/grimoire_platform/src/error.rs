//! Errors of the platform layer.

/// Failure of a platform runner.
#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    /// The native event loop could not be created or failed while running.
    #[error("event loop error: {0}")]
    EventLoop(String),
    /// The main window could not be created.
    #[error("window creation failed: {0}")]
    WindowCreation(String),
    /// [`crate::AppHandler::init`] returned an error.
    #[error("application init failed: {0}")]
    AppInit(String),
}
