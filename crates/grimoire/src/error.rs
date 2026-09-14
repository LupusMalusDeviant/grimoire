//! Errors of the facade.

use grimoire_platform::PlatformError;
use grimoire_render::RenderError;

/// Failure of an application run.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum GrimoireError {
    /// The platform runner failed: event loop, window creation or application init.
    #[error(transparent)]
    Platform(#[from] PlatformError),
    /// The renderer could not be created or failed while rendering.
    ///
    /// [`RenderError::SurfaceLost`] never ends a run; the frame is skipped and retried.
    #[error(transparent)]
    Render(#[from] RenderError),
}
