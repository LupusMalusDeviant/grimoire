//! Shared GPU context setup: an offscreen context requesting the adapter's own limits (not
//! `GpuContext::new_offscreen`'s conservative WebGL2 baseline), the same shape
//! `grimoire_gpu`'s own WP3.1 downlevel test uses — needed here for the light-culling spike's
//! storage buffers and compute pass.

use grimoire_gpu::{ContextOptions, GpuContext, GpuError, wgpu};

/// An offscreen context with the adapter's real limits. Honours `GRIMOIRE_GPU_ADAPTER` (the CI
/// workflow for this spike sets it to `software`, forcing WARP/lavapipe — never the developer's
/// hardware GPU).
///
/// # Errors
/// Whatever `GpuContext::new_offscreen_with_limits` returns (no adapter, or the adapter refused
/// the requested limits).
pub fn elevated_context() -> Result<GpuContext, GpuError> {
    let options = ContextOptions {
        allow_software_fallback: true,
        ..ContextOptions::default()
    };
    GpuContext::new_offscreen_with_limits(options, wgpu::Features::empty(), |limits| limits)
}
