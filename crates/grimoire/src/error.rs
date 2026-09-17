//! Errors of the facade.

use grimoire_platform::PlatformError;
use grimoire_render::RenderError;

use crate::render_assets::PluginError;

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
    /// A plugin's [`crate::GamePlugin::register_assets`] failed; the run ended before its first
    /// frame (contract §9.2).
    #[error("plugin `{plugin}` failed to register its assets: {source}")]
    Assets {
        /// [`crate::GamePlugin::name`] of the failing plugin.
        plugin: String,
        /// The plugin's error.
        #[source]
        source: PluginError,
    },
}
